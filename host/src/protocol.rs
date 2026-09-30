//! Control protocol: one JSON object per line in each direction.

use serde::{Deserialize, Serialize};
use stepwave_core::profile::MAX_GAIN_DB;
use stepwave_core::select;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeArg {
    Auto,
    Model,
    Eq,
    Bypass,
}

impl ModeArg {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeArg::Auto => "auto",
            ModeArg::Model => "model",
            ModeArg::Eq => "eq",
            ModeArg::Bypass => "bypass",
        }
    }
}

impl From<ModeArg> for select::Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Auto => select::Mode::Auto,
            ModeArg::Model => select::Mode::Model,
            ModeArg::Eq => select::Mode::Eq,
            ModeArg::Bypass => select::Mode::Bypass,
        }
    }
}

/// A client request. `{"cmd":"profile","value":null}` returns profile selection to auto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "value", rename_all = "lowercase")]
pub enum Request {
    Status,
    On,
    Off,
    Toggle,
    Mode(ModeArg),
    Strength(f32),
    Profile(Option<String>),
    Reload,
}

impl Request {
    /// Checks that do not need daemon state (profile names are checked by the daemon).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Request::Strength(v) if !v.is_finite() || !(0.0..=MAX_GAIN_DB).contains(v) => {
                Err(format!("strength {v} must be in [0, {MAX_GAIN_DB}] dB"))
            }
            _ => Ok(()),
        }
    }

    pub fn parse(line: &str) -> Result<Request, String> {
        let req: Request =
            serde_json::from_str(line.trim()).map_err(|e| format!("bad request: {e}"))?;
        req.validate()?;
        Ok(req)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutedStream {
    pub id: u32,
    pub binary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub enabled: bool,
    pub mode: ModeArg,
    pub strength_db: Option<f32>,
    pub profile: Option<String>,
    pub profile_pinned: bool,
    /// What is actually running: "model", "eq" or "bypass".
    pub processing: String,
    pub fallback_reason: Option<String>,
    /// Negotiated stream rate (always 48000 once audio flows; PipeWire converts
    /// from other graph rates), 0 until negotiated.
    pub graph_rate: u32,
    pub routed_streams: Vec<RoutedStream>,
    /// Audio device details; only the Windows app reports these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io: Option<IoStatus>,
}

/// Windows-app device state: which endpoints are open and how the drift compensation is doing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IoStatus {
    /// Capture endpoint (VB-Cable's "CABLE Output"), or None while it is missing.
    pub capture_device: Option<String>,
    /// Render endpoint (the current default output), or None while it is missing.
    pub render_device: Option<String>,
    /// Frames waiting between capture and render.
    pub ring_fill_frames: u32,
    /// Current drift correction applied on the render side, in ppm.
    pub drift_ppm: f64,
    /// Times the ring was resynced after leaving its safe range.
    pub resyncs: u64,
}

impl Status {
    /// Human-readable multi-line form printed by the CLIs.
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "enabled:    {}\nprofile:    {}{}\nmode:       {} (running: {})\nstrength:   {}\n",
            if self.enabled { "on" } else { "off (bypass)" },
            self.profile.as_deref().unwrap_or("-"),
            if self.profile_pinned { " (pinned)" } else { "" },
            self.mode.as_str(),
            self.processing,
            self.strength_db
                .map_or("-".to_string(), |v| format!("{v:.1} dB")),
        );
        if let Some(reason) = &self.fallback_reason {
            out.push_str(&format!("note:       {reason}\n"));
        }
        out.push_str(&format!(
            "rate:       {}\n",
            if self.graph_rate == 0 {
                "not negotiated yet (no audio)".to_string()
            } else {
                format!("{} Hz", self.graph_rate)
            }
        ));
        match &self.io {
            Some(io) => {
                let dev = |d: &Option<String>| d.clone().unwrap_or_else(|| "(not open)".into());
                out.push_str(&format!(
                    "capture:    {}\nrender:     {}\nbuffer:     {} frames, drift {:+.1} ppm, resyncs {}\n",
                    dev(&io.capture_device),
                    dev(&io.render_device),
                    io.ring_fill_frames,
                    io.drift_ppm,
                    io.resyncs,
                ));
            }
            None if self.routed_streams.is_empty() => out.push_str("routed:     none\n"),
            None => {
                for r in &self.routed_streams {
                    out.push_str(&format!("routed:     {} (node {})\n", r.binary, r.id));
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(status: Status) -> Self {
        Response {
            ok: true,
            status: Some(status),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Response {
            ok: false,
            status: None,
            error: Some(msg.into()),
        }
    }

    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).expect("Response always serialises");
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        let cases = [
            (r#"{"cmd":"status"}"#, Request::Status),
            (r#"{"cmd":"on"}"#, Request::On),
            (r#"{"cmd":"off"}"#, Request::Off),
            (r#"{"cmd":"toggle"}"#, Request::Toggle),
            (r#"{"cmd":"mode","value":"eq"}"#, Request::Mode(ModeArg::Eq)),
            (r#"{"cmd":"strength","value":7.0}"#, Request::Strength(7.0)),
            (
                r#"{"cmd":"profile","value":"cs2"}"#,
                Request::Profile(Some("cs2".into())),
            ),
            (r#"{"cmd":"profile","value":null}"#, Request::Profile(None)),
            (r#"{"cmd":"reload"}"#, Request::Reload),
        ];
        for (line, want) in cases {
            assert_eq!(Request::parse(line).unwrap(), want, "{line}");
        }
    }

    #[test]
    fn requests_round_trip() {
        for req in [
            Request::Toggle,
            Request::Mode(ModeArg::Bypass),
            Request::Strength(0.0),
            Request::Profile(None),
        ] {
            let line = serde_json::to_string(&req).unwrap();
            assert_eq!(Request::parse(&line).unwrap(), req);
        }
    }

    #[test]
    fn rejects_bad_requests() {
        for line in [
            r#"{"cmd":"strength","value":-1}"#,
            r#"{"cmd":"strength","value":30}"#,
            r#"{"cmd":"mode","value":"loud"}"#,
            r#"{"cmd":"explode"}"#,
            "not json",
        ] {
            assert!(Request::parse(line).is_err(), "{line}");
        }
    }

    fn sample_status() -> Status {
        Status {
            enabled: true,
            mode: ModeArg::Auto,
            strength_db: Some(7.0),
            profile: Some("cs2".into()),
            profile_pinned: false,
            processing: "model".into(),
            fallback_reason: None,
            graph_rate: 48_000,
            routed_streams: vec![RoutedStream {
                id: 7,
                binary: "cs2".into(),
            }],
            io: None,
        }
    }

    #[test]
    fn text_form_shows_routing_on_linux_and_devices_on_windows() {
        let linux = sample_status().to_text();
        assert!(linux.contains("routed:     cs2 (node 7)"), "{linux}");
        let mut win = sample_status();
        win.routed_streams.clear();
        win.io = Some(IoStatus {
            capture_device: Some("CABLE Output (VB-Audio Virtual Cable)".into()),
            render_device: None,
            ring_fill_frames: 1024,
            drift_ppm: -12.5,
            resyncs: 0,
        });
        let text = win.to_text();
        assert!(text.contains("capture:    CABLE Output"), "{text}");
        assert!(text.contains("render:     (not open)"), "{text}");
        assert!(
            text.contains("buffer:     1024 frames, drift -12.5 ppm, resyncs 0"),
            "{text}"
        );
        assert!(!text.contains("routed:"), "{text}");
    }

    #[test]
    fn io_is_omitted_from_json_when_absent() {
        let json = serde_json::to_string(&sample_status()).unwrap();
        assert!(!json.contains("\"io\""), "{json}");
    }

    #[test]
    fn response_is_one_line_and_omits_empty_fields() {
        let line = Response::err("nope").to_line();
        assert_eq!(line, "{\"ok\":false,\"error\":\"nope\"}\n");
    }
}
