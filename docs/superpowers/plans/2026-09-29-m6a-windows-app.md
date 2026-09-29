# M6a Windows App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `stepwave.exe` for Windows. It captures VB-Cable via WASAPI, processes the audio with `core`, renders it to the real device with clock-drift compensation, and is controlled by M5's CLI over a named pipe. The work also moves M5's platform-neutral runtime into a shared `host` crate.

**Architecture:**
- **`host`** (new): M5's `audio`, `engine`, `protocol` and `profiles` modules, moved out of `daemon`. The Linux daemon re-exports them unchanged.
- **`winapp`** (new):
  - Platform-neutral modules, tested on Linux and Windows: `drift` (`rubato::Slip` plus a PI controller with an adaptive target), `pipe` (an `interprocess` local socket, which is a named pipe on Windows) and `install` (Task Scheduler XML).
  - Windows-only modules: `wasapi_io` (capture and render threads) and `app` (wiring), plus the CLI.
- **CI:** a new `windows-latest` job compiles, lints and tests the Windows code on every push to the PR branch.

**Tech Stack:** Rust 1.98 (edition 2021). New crates: `rubato` 5, `interprocess` 2.4, `wasapi` 0.24 (Windows), `windows` 0.62 (Windows, for MMCSS) and `rtrb` 0.4. VB-Cable on the user's PC.

**Spec:** `docs/superpowers/specs/2026-09-29-m6a-windows-app-design.md` (amended during planning: `Slip`, an adaptive ring target, about 50–60 ms latency, format autoconvert, and best-effort pipe timeouts).

**How this code was verified.** Every file below was built in a scratch clone:
- **Linux:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings` are clean, and all 145 workspace tests pass.
- **Windows:** the code was pushed to a throwaway branch (since deleted) and run on GitHub `windows-latest`. `cargo clippy -p stepwave-core -p stepwave-host -p stepwave-winapp --all-targets -D warnings` is clean, the tests pass, and `cargo build --release -p stepwave-winapp` produces `stepwave.exe`.
- **Two Windows-specific facts came out of that run and are already in the code:**
  - Windows named pipes reject I/O timeouts, so the timeouts are best-effort.
  - A pipe write blocks until the peer reads, so the oversized-request test writes and reads on split halves.
- **Not verified:** the WASAPI glue has been compiled and linted but never run against real audio devices. The manual checklist (Task 5) covers that on the user's PC.

## Global Constraints

- Rust stable, edition 2021.
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` must pass on Linux after every task. The toolchain's clippy rejects `chunks_exact(<const>)`; use `as_chunks::<N>()` instead.
- Once `winapp` exists (Task 2 onward), the CI `windows` job must also pass.
- **Branch workflow:**
  - All implementation commits go to the branch `m6a-windows`, which the controller has already created. Never commit to `main`.
  - After each task's commit, run `git push origin m6a-windows`, wait for the draft PR's CI run (`gh pr checks --watch` or `gh run watch <id> --exit-status`), and include the result in the report.
  - The Windows job is the only way to compile `#[cfg(windows)]` code, so a task is not done until it is green.
- Real-time rules (CLAUDE.md rule 2): `drift::Capture::push`, `drift::Render::render`, `AudioCore::process_interleaved` and the per-packet loops in `wasapi_io` never allocate, lock, log or panic. Buffers are allocated before each loop.
- Stereo image: `core::Processor` applies one mask to L and R. Nothing here changes that.
- `core` always processes 48 kHz. WASAPI streams are opened as 48 kHz float stereo with engine autoconvert; drift compensation happens after `core`.
- Drift compensation: `rubato::Slip` with 64-frame chunks. The target is `max(2 · max_period + 64, 256)` frames. PI gains are KP = 2.0 and KI = 0.07, the fill smoothing is 0.0027 per chunk, the correction is clamped to ±1000 ppm, and a resync happens above 4 × target or on running dry.
- Control: M5's JSON-line protocol, unchanged. The pipe name is `stepwave`. Status gains an optional `io` object (`IoStatus`), which the Linux daemon omits.
- Files live under `%LOCALAPPDATA%\stepwave\` (`profiles\`, `models\`). The default profile is `cs2`, else the first loaded profile.
- Test data is synthetic only.
- Commits are conventional (`feat(winapp): …`) and each message ends with the trailer line `Co-Authored-By: Claude <noreply@anthropic.com>`.

## Review Focus

- **Clocks drifting apart for a long session:** no clicks, resyncs or growing latency. Covered in Task 2 (`tracks_drift_without_resyncs_or_clicks`, 10 min × 5 clock pairs) and the 30-minute manual check (Task 5).
- **VB-Cable missing or the capture device disappearing:** the app keeps running, `status` explains it, and it retries. Covered in Task 4 (`capture_thread` backoff, with `capture_error` surfaced in `status`) and manual check 8.
- **Default output device changes mid-game:** the render stream reopens on the new device. Covered in Task 4 (`render_generation` bumped by the main loop) and manual check 3.
- **A second `stepwave run`, or a client that never finishes its request:** the second instance is refused, and other commands are not blocked. Covered in Task 3 (`not_running_and_second_instance`, `silent_client_does_not_block_others`).
- **A capture or render stall (system hiccup):** one resync, then recovery, with no panic. Covered in Task 2 (`a_render_stall_causes_one_resync`, `a_capture_stall_primes_again_and_recovers`).

## File Map

| File | Responsibility |
|---|---|
| `host/Cargo.toml`, `host/src/lib.rs` | Shared runtime crate |
| `host/src/{audio,engine,profiles,protocol}.rs` | Moved from `daemon/src/` (protocol and engine gain small additions) |
| `host/tests/no_alloc.rs` | Moved from `daemon/tests/` |
| `daemon/src/lib.rs`, `daemon/Cargo.toml`, `daemon/src/main.rs`, `daemon/src/control.rs` | Re-export `host`; CLI uses `Status::to_text` |
| `winapp/Cargo.toml`, `winapp/src/lib.rs`, `winapp/src/main.rs` | Windows app crate and CLI |
| `winapp/src/drift.rs` | Slip + PI drift compensation, adaptive target |
| `winapp/src/pipe.rs` | Control pipe server and client |
| `winapp/src/install.rs` | Task Scheduler logon task |
| `winapp/src/wasapi_io.rs` | WASAPI capture and render threads (Windows) |
| `winapp/src/app.rs` | `stepwave run` wiring (Windows) |
| `winapp/tests/no_alloc.rs` | Allocation counter over push and render |
| `.github/workflows/ci.yml` | New `windows` job |
| `docs/windows-setup.md`, `docs/measurements/m6a-checklist.md`, `README.md` | User docs |

---

### Task 1: Extract the shared `host` crate

**Files:**
- Create: `host/Cargo.toml`, `host/src/lib.rs`
- Move (with `git mv`, content unchanged unless listed): `daemon/src/audio.rs` → `host/src/audio.rs`, `daemon/src/engine.rs` → `host/src/engine.rs`, `daemon/src/profiles.rs` → `host/src/profiles.rs`, `daemon/src/protocol.rs` → `host/src/protocol.rs`, `daemon/tests/no_alloc.rs` → `host/tests/no_alloc.rs`
- Modify: root `Cargo.toml` (members), `daemon/Cargo.toml`, `daemon/src/lib.rs`, `daemon/src/main.rs`, `daemon/src/control.rs`, `host/src/protocol.rs`, `host/src/engine.rs`, `host/src/profiles.rs`, `host/tests/no_alloc.rs`
- Test: existing tests (now in `host`), plus new `protocol` tests (`text_form_…`, `io_is_omitted_…`) and a new `engine` test (`default_profile_…`)

**Interfaces:**
- Produces:
  - Crate `stepwave-host` with `pub mod audio; pub mod engine; pub mod profiles; pub mod protocol;`, and the same items as before.
  - `protocol::Status` gains `pub io: Option<IoStatus>` (serde default, skipped when `None`).
  - `protocol::IoStatus { capture_device: Option<String>, render_device: Option<String>, ring_fill_frames: u32, drift_ppm: f64, resyncs: u64 }`.
  - `Status::to_text(&self) -> String`, the CLI's human-readable form. When `io` is present it prints the device and buffer lines instead of `routed:`.
  - `Engine::default_profile(&self, preferred: &str) -> Option<String>`.
  - `ProfileSet::ids(&self) -> Vec<String>` (sorted).
  - `stepwave_daemon` re-exports `stepwave_host::{audio, engine, profiles, protocol}`, so every `crate::audio` and `stepwave_daemon::protocol` path in `daemon` keeps working.

- [ ] **Step 1: Move the files and create the crate.**

```bash
git switch m6a-windows
mkdir -p host/src host/tests
git mv daemon/src/audio.rs daemon/src/engine.rs daemon/src/protocol.rs daemon/src/profiles.rs host/src/
git mv daemon/tests/no_alloc.rs host/tests/
```

Root `Cargo.toml` members line:
```toml
members = ["core", "cli", "host", "daemon", "bindings/python"]
```

`host/Cargo.toml`:
```toml
[package]
name = "stepwave-host"
description = "Platform-neutral stepwave runtime: processor swaps, engine, control protocol, profiles"
version.workspace = true
edition.workspace = true

[dependencies]
rtrb = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
stepwave-core = { path = "../core" }

[dev-dependencies]
tempfile = "3"
```

`host/src/lib.rs`:
```rust
//! Platform-neutral stepwave runtime shared by the Linux daemon and the Windows app:
//! lock-free processor swaps (`audio`), control state (`engine`), the JSON-line control
//! protocol (`protocol`) and profile loading (`profiles`).

pub mod audio;
pub mod engine;
pub mod profiles;
pub mod protocol;
```

`daemon/src/lib.rs`:
```rust
//! stepwave Linux daemon: PipeWire sink running `core`, per-app routing, CLI control.

pub use stepwave_host::{audio, engine, profiles, protocol};

pub mod control;
pub mod daemon;
pub mod graph;
pub mod node;
pub mod router;
```

In `daemon/Cargo.toml`, add `stepwave-host = { path = "../host" }` after the `stepwave-core` line. Full file:
```toml
[package]
name = "stepwave-daemon"
description = "Linux stepwave daemon: PipeWire sink running core, per-app routing, CLI control"
version.workspace = true
edition.workspace = true

[[bin]]
name = "stepwave"
path = "src/main.rs"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
pipewire = { version = "0.10", features = ["v0_3_49"] }
rtrb = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
stepwave-core = { path = "../core" }
stepwave-host = { path = "../host" }

[dev-dependencies]
tempfile = "3"
```

In `host/tests/no_alloc.rs`, change `use stepwave_daemon::audio::{channel, WARMUP, XFADE};` to `use stepwave_host::audio::{channel, WARMUP, XFADE};`.

Run `cargo test -p stepwave-host -p stepwave-daemon`. Expected: every moved test passes (the move is behaviour-neutral).

- [ ] **Step 2: Write the failing tests for the additions.** Replace `host/src/protocol.rs`, `host/src/engine.rs` and `host/src/profiles.rs` with the final versions below. They contain both the new tests and the new code; to see RED first, temporarily delete `to_text`, `IoStatus`, `default_profile` and `ids` from them, run `cargo test -p stepwave-host` (compile errors), then restore them.

`host/src/protocol.rs`:
```rust
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
```

`host/src/engine.rs`:
```rust
//! Control-side state: which profile, mode, strength and on/off are wanted, and building
//! the matching `Processor` off the audio thread whenever that changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use stepwave_core::mask::UnityMask;
use stepwave_core::select;
use stepwave_core::stft::SAMPLE_RATE;
use stepwave_core::Processor;

use crate::audio::Handoff;
use crate::profiles::ProfileSet;
use crate::protocol::{ModeArg, Request, Response, RoutedStream, Status};

/// Everything a processor is built from; a new one is built only when this changes.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    profile: Option<String>,
    mode: ModeArg,
    strength: Option<f32>,
}

pub struct Engine {
    profiles_dir: PathBuf,
    profiles: ProfileSet,
    enabled: bool,
    mode: ModeArg,
    strength: Option<f32>,
    pinned: Option<String>,
    auto_profile: Option<String>,
    processing: String,
    fallback_reason: Option<String>,
    applied: Option<Key>,
    handoff: Handoff,
    pending: Option<Box<Processor>>,
}

/// The processor the audio thread starts with: unity, same latency as every other mode.
pub fn initial_processor() -> Processor {
    Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE).expect("48 kHz is supported")
}

impl Engine {
    /// Returns the engine and the warnings from loading the profiles.
    pub fn new(profiles_dir: &Path, mode: ModeArg, handoff: Handoff) -> (Engine, Vec<String>) {
        let (profiles, warnings) = ProfileSet::load(profiles_dir);
        let mut engine = Engine {
            profiles_dir: profiles_dir.to_path_buf(),
            profiles,
            enabled: true,
            mode,
            strength: None,
            pinned: None,
            auto_profile: None,
            processing: "bypass".into(),
            fallback_reason: Some("no game detected yet".into()),
            applied: None,
            handoff,
            pending: None,
        };
        engine.apply();
        (engine, warnings)
    }

    /// binary -> profile id, for the router.
    pub fn matches(&self) -> HashMap<String, String> {
        self.profiles.matches()
    }

    /// The router saw a new matching stream.
    pub fn select_profile(&mut self, id: &str) {
        if self.profiles.get(id).is_some() {
            self.auto_profile = Some(id.to_string());
            self.apply();
        }
    }

    /// A new audio session (after a PipeWire reconnect): republish the current state.
    pub fn replace_handoff(&mut self, handoff: Handoff) {
        self.handoff = handoff;
        self.pending = None;
        self.applied = None;
        self.apply();
    }

    /// Periodic housekeeping: drop retired processors, retry a publish that found the
    /// queue full.
    pub fn tick(&mut self) {
        self.handoff.collect();
        if let Some(p) = self.pending.take() {
            if let Err(p) = self.handoff.publish(p) {
                self.pending = Some(p);
            }
        }
    }

    /// Handle one request. `graph_rate` and `routed` come from the PipeWire side.
    pub fn handle(&mut self, req: Request, graph_rate: u32, routed: &[(u32, String)]) -> Response {
        if let Err(e) = req.validate() {
            return Response::err(e);
        }
        match req {
            Request::Status => {}
            Request::On => self.enabled = true,
            Request::Off => self.enabled = false,
            Request::Toggle => self.enabled = !self.enabled,
            Request::Mode(m) => self.mode = m,
            Request::Strength(s) => self.strength = Some(s),
            Request::Profile(Some(id)) => {
                if self.profiles.get(&id).is_none() {
                    return Response::err(format!("unknown profile '{id}'"));
                }
                self.pinned = Some(id);
            }
            Request::Profile(None) => self.pinned = None,
            Request::Reload => {
                let (profiles, warnings) = ProfileSet::load(&self.profiles_dir);
                for w in warnings {
                    eprintln!("stepwave: {w}");
                }
                self.profiles = profiles;
                // A CLI strength override lasts until reload/restart.
                self.strength = None;
                if self
                    .pinned
                    .as_ref()
                    .is_some_and(|id| self.profiles.get(id).is_none())
                {
                    self.pinned = None;
                }
                if self
                    .auto_profile
                    .as_ref()
                    .is_some_and(|id| self.profiles.get(id).is_none())
                {
                    self.auto_profile = None;
                }
                self.applied = None; // model files may have changed: rebuild
            }
        }
        self.apply();
        Response::ok(self.status(graph_rate, routed))
    }

    pub fn status(&self, graph_rate: u32, routed: &[(u32, String)]) -> Status {
        let profile = self.active_profile();
        let strength_db = self.strength.or_else(|| {
            profile
                .as_ref()
                .and_then(|id| self.profiles.get(id))
                .map(|p| p.profile.strength_db)
        });
        Status {
            enabled: self.enabled,
            mode: self.mode,
            strength_db,
            profile,
            profile_pinned: self.pinned.is_some(),
            processing: self.processing.clone(),
            fallback_reason: self.fallback_reason.clone(),
            graph_rate,
            routed_streams: routed
                .iter()
                .map(|(id, binary)| RoutedStream {
                    id: *id,
                    binary: binary.clone(),
                })
                .collect(),
            io: None,
        }
    }

    /// The profile to start with when nothing selects one: `preferred` if loaded, else the
    /// first loaded profile id in sorted order.
    pub fn default_profile(&self, preferred: &str) -> Option<String> {
        if self.profiles.get(preferred).is_some() {
            return Some(preferred.to_string());
        }
        self.profiles.ids().into_iter().next()
    }

    fn active_profile(&self) -> Option<String> {
        self.pinned.clone().or_else(|| self.auto_profile.clone())
    }

    fn apply(&mut self) {
        let key = Key {
            profile: self.active_profile(),
            mode: if self.enabled {
                self.mode
            } else {
                ModeArg::Bypass
            },
            strength: self.strength,
        };
        if self.applied.as_ref() == Some(&key) {
            return;
        }
        let (processor, processing, reason) = self.build(&key);
        self.processing = processing;
        self.fallback_reason = reason;
        self.applied = Some(key);
        // A newer processor supersedes one still waiting for queue space.
        self.pending = None;
        if let Err(p) = self.handoff.publish(Box::new(processor)) {
            self.pending = Some(p);
        }
    }

    fn build(&self, key: &Key) -> (Processor, String, Option<String>) {
        let unity = |reason: Option<String>| (initial_processor(), "bypass".to_string(), reason);
        let Some(id) = &key.profile else {
            return unity(Some("no game detected yet".into()));
        };
        let Some(loaded) = self.profiles.get(id) else {
            return unity(Some(format!("profile '{id}' not loaded")));
        };
        let mut profile = loaded.profile.clone();
        if let Some(s) = key.strength {
            profile.strength_db = s;
        }
        match select::build(key.mode.into(), &profile, &loaded.model_path, false) {
            Ok(b) => (
                b.processor,
                b.processing.as_str().to_string(),
                b.fallback_reason,
            ),
            Err(e) => unity(Some(format!("profile '{id}' unusable ({e}); bypassing"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::channel;

    const CS2: &str = include_str!("../../profiles/cs2.json");
    const FIXTURE: &[u8] = include_bytes!("../../core/tests/fixtures/model_random.swm");

    /// `<tmp>/profiles/cs2.json` (+ `<tmp>/models/cs2.swm` when `with_model`).
    fn setup(with_model: bool) -> (tempfile::TempDir, Engine, crate::audio::AudioCore) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("profiles")).unwrap();
        std::fs::write(root.path().join("profiles/cs2.json"), CS2).unwrap();
        if with_model {
            std::fs::create_dir(root.path().join("models")).unwrap();
            std::fs::write(root.path().join("models/cs2.swm"), FIXTURE).unwrap();
        }
        let (handoff, core) = channel(initial_processor());
        let (engine, warnings) = Engine::new(&root.path().join("profiles"), ModeArg::Auto, handoff);
        assert!(warnings.is_empty(), "{warnings:?}");
        (root, engine, core)
    }

    fn st(e: &mut Engine, req: Request) -> Status {
        let r = e.handle(req, 48_000, &[]);
        assert!(r.ok, "{:?}", r.error);
        r.status.unwrap()
    }

    #[test]
    fn default_profile_prefers_the_named_one_then_the_first() {
        let (d, e, _c) = setup(true);
        assert_eq!(e.default_profile("cs2").as_deref(), Some("cs2"));
        assert_eq!(e.default_profile("pubg").as_deref(), Some("cs2"));
        drop(d);
    }

    #[test]
    fn starts_in_bypass_until_a_game_is_seen() {
        let (_d, mut e, _c) = setup(true);
        let s = st(&mut e, Request::Status);
        assert_eq!(s.processing, "bypass");
        assert_eq!(s.profile, None);
        assert_eq!(e.matches().get("cs2").map(String::as_str), Some("cs2"));
    }

    #[test]
    fn selecting_a_profile_builds_the_model_and_toggle_bypasses() {
        let (_d, mut e, _c) = setup(true);
        e.select_profile("cs2");
        let s = st(&mut e, Request::Status);
        assert_eq!(
            (s.processing.as_str(), s.profile.as_deref()),
            ("model", Some("cs2"))
        );
        assert_eq!(s.strength_db, Some(7.0));
        let s = st(&mut e, Request::Toggle);
        assert!(!s.enabled);
        assert_eq!(s.processing, "bypass");
        let s = st(&mut e, Request::Toggle);
        assert_eq!(s.processing, "model");
    }

    #[test]
    fn missing_model_falls_back_to_eq_with_a_reason() {
        let (_d, mut e, _c) = setup(false);
        e.select_profile("cs2");
        let s = st(&mut e, Request::Mode(ModeArg::Model));
        assert_eq!(s.processing, "eq");
        assert!(s.fallback_reason.unwrap().contains("cs2.swm"));
    }

    #[test]
    fn strength_mode_and_pin_are_applied_and_validated() {
        let (_d, mut e, _c) = setup(true);
        let s = st(&mut e, Request::Profile(Some("cs2".into())));
        assert!(s.profile_pinned);
        let s = st(&mut e, Request::Strength(3.5));
        assert_eq!(s.strength_db, Some(3.5));
        let s = st(&mut e, Request::Mode(ModeArg::Eq));
        assert_eq!(s.processing, "eq");
        assert!(
            !e.handle(Request::Profile(Some("nope".into())), 48_000, &[])
                .ok
        );
        assert!(!e.handle(Request::Strength(99.0), 48_000, &[]).ok);
        let s = st(&mut e, Request::Profile(None));
        assert!(!s.profile_pinned);
    }

    #[test]
    fn reload_picks_up_new_profiles_and_drops_removed_ones() {
        let (d, mut e, _c) = setup(true);
        e.select_profile("cs2");
        std::fs::remove_file(d.path().join("profiles/cs2.json")).unwrap();
        let s = st(&mut e, Request::Reload);
        assert_eq!(s.profile, None);
        assert_eq!(s.processing, "bypass");
    }

    #[test]
    fn reload_clears_the_strength_override() {
        let (_d, mut e, _c) = setup(true);
        e.select_profile("cs2");
        let s = st(&mut e, Request::Strength(3.5));
        assert_eq!(s.strength_db, Some(3.5));
        let s = st(&mut e, Request::Reload);
        assert_eq!(s.strength_db, Some(7.0), "back to the profile's strength");
    }

    #[test]
    fn status_reports_routed_streams_and_rate() {
        let (_d, e, _c) = setup(true);
        let s = e.status(44_100, &[(7, "cs2".into())]);
        assert_eq!(s.graph_rate, 44_100);
        assert_eq!(
            s.routed_streams,
            vec![RoutedStream {
                id: 7,
                binary: "cs2".into()
            }]
        );
    }

    #[test]
    fn rapid_changes_never_lose_the_latest_processor() {
        let (_d, mut e, mut core) = setup(true);
        e.select_profile("cs2");
        for _ in 0..5 {
            st(&mut e, Request::Toggle);
        }
        // Drain the audio side, then tick: the pending (latest) processor gets published.
        let mut block = vec![0.0f32; 4096];
        for _ in 0..10 {
            core.process_interleaved(&mut block);
            e.tick();
        }
        assert!(e.pending.is_none());
    }
}
```

`host/src/profiles.rs`:
```rust
//! Loading the profiles directory. Invalid profiles are skipped with a warning.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use stepwave_core::select::resolve_model_path;
use stepwave_core::Profile;

#[derive(Debug, Clone)]
pub struct LoadedProfile {
    pub profile: Profile,
    pub model_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileSet {
    by_id: BTreeMap<String, LoadedProfile>,
}

impl ProfileSet {
    /// Reads every `*.json` in `dir`. Returns the set and one warning per skipped file.
    pub fn load(dir: &Path) -> (ProfileSet, Vec<String>) {
        let mut set = ProfileSet::default();
        let mut warnings = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                warnings.push(format!("profiles dir {}: {e}", dir.display()));
                return (set, warnings);
            }
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|json| Profile::from_json(&json).map_err(|e| e.to_string()));
            match parsed {
                Ok(profile) => {
                    let model_path = resolve_model_path(&profile, &path);
                    set.by_id.insert(
                        profile.id.clone(),
                        LoadedProfile {
                            profile,
                            model_path,
                        },
                    );
                }
                Err(e) => warnings.push(format!("skipping profile {}: {e}", path.display())),
            }
        }
        (set, warnings)
    }

    pub fn get(&self, id: &str) -> Option<&LoadedProfile> {
        self.by_id.get(id)
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Loaded profile ids, sorted.
    pub fn ids(&self) -> Vec<String> {
        self.by_id.keys().cloned().collect()
    }

    /// Process binary name -> profile id, from every profile's `match.linux`.
    pub fn matches(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for p in self.by_id.values() {
            for b in &p.profile.match_rules.linux {
                m.insert(b.clone(), p.profile.id.clone());
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CS2: &str = include_str!("../../profiles/cs2.json");

    #[test]
    fn loads_valid_skips_invalid_and_resolves_models() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("profiles");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("cs2.json"), CS2).unwrap();
        std::fs::write(dir.join("broken.json"), "{ nope").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let (set, warnings) = ProfileSet::load(&dir);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("broken.json"));
        let cs2 = set.get("cs2").unwrap();
        assert_eq!(cs2.model_path, root.path().join("models/cs2.swm"));
        let m = set.matches();
        assert_eq!(m.get("cs2").map(String::as_str), Some("cs2"));
        assert_eq!(m.get("firefox"), None);
    }

    #[test]
    fn missing_dir_is_a_warning_not_a_panic() {
        let (set, warnings) = ProfileSet::load(Path::new("/nonexistent/profiles"));
        assert!(set.is_empty());
        assert_eq!(warnings.len(), 1);
    }
}
```

- [ ] **Step 3: Update the daemon for the new field and the shared formatter.** In `daemon/src/control.rs`'s test helper `status()`, add `io: None,` after `routed_streams: vec![],`. Replace `daemon/src/main.rs` with:

```rust
//! `stepwave`: the Linux daemon and its control commands.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use stepwave_daemon::control;
use stepwave_daemon::daemon::{self, Options};
use stepwave_daemon::protocol::{ModeArg, Request, Response};

#[derive(Parser)]
#[command(
    name = "stepwave",
    version,
    about = "stepwave footstep enhancer for PipeWire"
)]
struct Cli {
    /// Control socket (default: $XDG_RUNTIME_DIR/stepwave.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print the daemon's raw JSON reply
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (normally started by systemd)
    Daemon {
        /// Profiles directory (default: ~/.config/stepwave/profiles)
        #[arg(long)]
        profiles: Option<PathBuf>,
        /// Initial mode
        #[arg(long, value_enum, default_value = "auto")]
        mode: CliMode,
    },
    /// Show what the daemon is doing
    Status,
    /// Enable processing
    On,
    /// Bypass processing (latency unchanged, for fair A/B)
    Off,
    /// Flip on/off (bind this to a global shortcut)
    Toggle,
    /// Processing mode
    Mode {
        #[arg(value_enum)]
        mode: CliMode,
    },
    /// Override the active profile's strength (dB, 0..24) until reload/restart
    Strength { db: f32 },
    /// Pin a profile, or `--auto` to follow the detected game again
    Profile {
        #[arg(required_unless_present = "auto")]
        id: Option<String>,
        #[arg(long, conflicts_with = "id")]
        auto: bool,
    },
    /// Re-read profiles and models from disk
    Reload,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CliMode {
    Auto,
    Model,
    Eq,
    Bypass,
}

impl From<CliMode> for ModeArg {
    fn from(m: CliMode) -> Self {
        match m {
            CliMode::Auto => ModeArg::Auto,
            CliMode::Model => ModeArg::Model,
            CliMode::Eq => ModeArg::Eq,
            CliMode::Bypass => ModeArg::Bypass,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let socket = match cli
        .socket
        .clone()
        .map(Ok)
        .unwrap_or_else(control::default_socket_path)
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stepwave: {e}");
            return ExitCode::FAILURE;
        }
    };
    let request = match cli.command {
        Command::Daemon { profiles, mode } => {
            let profiles_dir = profiles.unwrap_or_else(default_profiles_dir);
            let opts = Options {
                profiles_dir,
                mode: mode.into(),
                socket,
            };
            return match daemon::run(opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("stepwave: {e:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Status => Request::Status,
        Command::On => Request::On,
        Command::Off => Request::Off,
        Command::Toggle => Request::Toggle,
        Command::Mode { mode } => Request::Mode(mode.into()),
        Command::Strength { db } => Request::Strength(db),
        Command::Profile { id, .. } => Request::Profile(id),
        Command::Reload => Request::Reload,
    };
    match control::request(&socket, &request) {
        Ok(resp) => print_response(&resp, cli.json),
        Err(e) => {
            eprintln!("stepwave: {e}");
            ExitCode::FAILURE
        }
    }
}

fn default_profiles_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config.join("stepwave/profiles")
}

fn print_response(resp: &Response, json: bool) -> ExitCode {
    if json {
        print!("{}", resp.to_line());
    } else if let Some(err) = &resp.error {
        eprintln!("stepwave: {err}");
    } else if let Some(status) = &resp.status {
        print!("{}", status.to_text());
    }
    if resp.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
```

- [ ] **Step 4: Verify.** Run `cargo test -p stepwave-host -p stepwave-daemon && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 22 host lib tests and 1 host no_alloc test pass, 15 daemon tests pass, and the 5 PipeWire tests show as ignored. Optionally run the ignored PipeWire tests once (`cargo test -p stepwave-daemon --test pipewire -- --ignored --test-threads=1`, which plays a quiet tone) to confirm the daemon still works.

- [ ] **Step 5: Commit and push.**

```bash
git add -A
git commit -m "refactor(host): move the shared runtime out of the Linux daemon

audio (processor swaps), engine, protocol and profiles move to a
platform-neutral stepwave-host crate so the Windows app can reuse
them; the daemon re-exports them unchanged. Status gains an optional
io object for device/drift details, Status::to_text replaces the
daemon CLI's private formatter, and Engine::default_profile picks a
start profile when nothing selects one.

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin m6a-windows
```

(The controller opens the draft PR after this push; CI runs from then on.)

---

### Task 2: `winapp` crate, drift compensation and the Windows CI job

**Files:**
- Create: `winapp/Cargo.toml`, `winapp/src/lib.rs` (Task 2 version), `winapp/src/main.rs` (a stub), `winapp/src/drift.rs`, `winapp/tests/no_alloc.rs`
- Modify: root `Cargo.toml` (members), `.github/workflows/ci.yml`
- Test: unit tests in `drift.rs`, plus `winapp/tests/no_alloc.rs`

**Interfaces:**
- Consumes: nothing from `winapp` yet. `host` and `core` are dependencies for later tasks.
- Produces:
  - `drift::{CHANNELS = 2, CHUNK = 64, MIN_TARGET = 256, RESYNC_FACTOR = 4, MAX_PPM = 1000.0, RING_FRAMES = 48_000}`
  - `drift::channel() -> (Capture, Render, Arc<DriftStats>)`
  - `Capture::push(&mut self, interleaved: &[f32]) -> usize` (frames pushed; whole frames only)
  - `Render::render(&mut self, out: &mut [f32])` (fills completely) and `Render::reset(&mut self)`
  - `DriftStats::{fill_frames() -> u32, target_frames() -> u32, ppm() -> f64, resyncs() -> u64}`

- [ ] **Step 1: Scaffold.** Root `Cargo.toml` members:
```toml
members = ["core", "cli", "host", "daemon", "winapp", "bindings/python"]
```

`winapp/Cargo.toml`:
```toml
[package]
name = "stepwave-winapp"
description = "Windows stepwave app: VB-Cable capture, core processing, WASAPI render, CLI control"
version.workspace = true
edition.workspace = true

[[bin]]
name = "stepwave"
path = "src/main.rs"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
interprocess = "2.4"
rtrb = "0.4"
rubato = "5"
serde_json = "1"
stepwave-core = { path = "../core" }
stepwave-host = { path = "../host" }

[target.'cfg(windows)'.dependencies]
wasapi = "0.24"
windows = { version = "0.62", features = ["Win32_Foundation", "Win32_System_Threading"] }

[dev-dependencies]
tempfile = "3"
```

`winapp/src/lib.rs` (Task 2 version; Tasks 3–4 add modules):
```rust
//! stepwave for Windows: VB-Cable capture → core → WASAPI render, CLI over a named pipe.

pub mod drift;
```

`winapp/src/main.rs` (stub until Task 4):
```rust
fn main() {}
```

Replace `.github/workflows/ci.yml` with (it adds the `windows` job; the other jobs are unchanged):
```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      # pipewire-sys (daemon crate) needs the PipeWire headers and libclang for bindgen.
      - run: sudo apt-get update && sudo apt-get install -y libpipewire-0.3-dev libclang-dev
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      - run: cargo bench --workspace --no-run

  windows:
    runs-on: windows-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      # Only the crates that build on Windows (the Linux daemon needs PipeWire).
      - run: cargo clippy -p stepwave-core -p stepwave-host -p stepwave-winapp --all-targets -- -D warnings
      - run: cargo test -p stepwave-core -p stepwave-host -p stepwave-winapp
      - run: cargo build --release -p stepwave-winapp
      - uses: actions/upload-artifact@v4
        with:
          name: stepwave-windows
          path: target/release/stepwave.exe

  python:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: training/datagen
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: astral-sh/setup-uv@v6
      - run: uv sync --locked
      - run: uv run ruff check
      - run: uv run ruff format --check
      - run: uv run pytest -q

  model:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: training/model
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: astral-sh/setup-uv@v6
      - run: uv sync --locked --extra audio
      - run: uv run ruff check
      - run: uv run ruff format --check
      - run: uv run pytest -q
```

- [ ] **Step 2: Write the failing tests.** Create `winapp/src/drift.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    struct Sim {
        left: Vec<f32>,
        stats: Arc<DriftStats>,
        fills: Vec<usize>,
        ppms: Vec<f64>,
    }

    type Clock = (f64, usize); // (ppm offset, frames per period)
    type Window = Option<(f64, f64)>; // (start s, length s)

    /// Simulate `seconds` of audio with the given capture and render clocks. `ren_stall`
    /// freezes the render side and `cap_stall` the capture side for a window.
    fn simulate(seconds: f64, cap: Clock, ren: Clock, ren_stall: Window, cap_stall: Window) -> Sim {
        let (mut capture, mut render, stats) = channel();
        let cap_period = cap.1 as f64 / (RATE * (1.0 + cap.0 * 1e-6));
        let ren_period = ren.1 as f64 / (RATE * (1.0 + ren.0 * 1e-6));
        let (mut t_cap, mut t_ren) = (0.0f64, 0.0037f64);
        let mut phase = 0u64;
        let mut packet = vec![0.0f32; cap.1 * CHANNELS];
        let mut out = vec![0.0f32; ren.1 * CHANNELS];
        let (mut left, mut fills, mut ppms) = (Vec::new(), Vec::new(), Vec::new());
        let frozen = |w: Window, t: f64| w.is_some_and(|(s, l)| t >= s && t < s + l);
        while t_cap.min(t_ren) < seconds {
            if t_cap <= t_ren {
                if !frozen(cap_stall, t_cap) {
                    for f in packet.as_chunks_mut::<CHANNELS>().0 {
                        let x = 2.0 * std::f64::consts::PI * 1000.0 * phase as f64 / RATE;
                        *f = [(0.5 * x.sin()) as f32; CHANNELS];
                        phase += 1;
                    }
                    capture.push(&packet);
                }
                t_cap += cap_period;
            } else {
                if !frozen(ren_stall, t_ren) {
                    render.render(&mut out);
                    left.extend(out.as_chunks::<CHANNELS>().0.iter().map(|f| f[0]));
                    fills.push(render.fill());
                    ppms.push(stats.ppm());
                }
                t_ren += ren_period;
            }
        }
        Sim {
            left,
            stats,
            fills,
            ppms,
        }
    }

    fn max_step(x: &[f32]) -> f32 {
        x.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn tracks_drift_without_resyncs_or_clicks() {
        // 10 minutes each. The 1 kHz, 0.5-amplitude sine's natural max step is
        // 0.5·2π·1000/48000 ≈ 0.0654.
        let cases = [
            ((0.0, 480), (0.0, 480)),
            ((200.0, 480), (0.0, 480)),
            ((-200.0, 480), (0.0, 480)),
            ((0.0, 480), (200.0, 480)),
            ((0.0, 144), (-150.0, 441)),
        ];
        for (cap, ren) in cases {
            let sim = simulate(600.0, cap, ren, None, None);
            let tag = format!("cap {cap:?} ren {ren:?}");
            assert_eq!(sim.stats.resyncs(), 0, "{tag}");
            let settled = &sim.fills[sim.fills.len() / 60..]; // after the first 10 s
            assert!(settled.iter().all(|&f| f > 0), "{tag}: ran dry");
            let step = max_step(&sim.left[48_000..]);
            assert!(step < 0.08, "{tag}: step {step}");
            assert!(sim.stats.ppm().abs() < MAX_PPM, "{tag}: saturated");
        }
    }

    #[test]
    fn converges_on_the_clock_difference() {
        // Render clock 200 ppm faster than capture: once settled the render side consumes
        // ~200 ppm less input per output frame, i.e. ppm ≈ -200.
        // The instantaneous value wobbles with the packet phase, so average the last minute.
        let sim = simulate(240.0, (0.0, 480), (200.0, 480), None, None);
        let last_minute = &sim.ppms[sim.ppms.len() - 6000..];
        let mean = last_minute.iter().sum::<f64>() / last_minute.len() as f64;
        assert!((mean + 200.0).abs() < 20.0, "mean ppm {mean}");
    }

    #[test]
    fn target_adapts_to_packet_size() {
        let sim = simulate(2.0, (0.0, 480), (0.0, 480), None, None);
        assert_eq!(sim.stats.target_frames() as usize, 2 * 480 + CHUNK);
        let sim = simulate(2.0, (0.0, 96), (0.0, 96), None, None);
        assert_eq!(sim.stats.target_frames() as usize, MIN_TARGET);
    }

    #[test]
    fn a_render_stall_causes_one_resync() {
        let sim = simulate(20.0, (0.0, 480), (0.0, 480), Some((5.0, 0.5)), None);
        assert_eq!(sim.stats.resyncs(), 1);
    }

    #[test]
    fn a_capture_stall_primes_again_and_recovers() {
        let sim = simulate(20.0, (0.0, 480), (0.0, 480), None, Some((5.0, 0.3)));
        assert_eq!(sim.stats.resyncs(), 1);
        let tail = &sim.fills[sim.fills.len() - 100..];
        assert!(tail.iter().all(|&f| f > 0));
    }

    #[test]
    fn render_primes_to_the_target_before_playing() {
        let (mut cap, mut ren, stats) = channel();
        let mut out = vec![1.0f32; CHUNK * CHANNELS];
        // 100-frame packets and 64-frame requests: target = max(2·100 + 64, 256) = 264.
        for _ in 0..2 {
            cap.push(&vec![0.5f32; 100 * CHANNELS]);
        }
        cap.push(&vec![0.5f32; 63 * CHANNELS]);
        ren.render(&mut out);
        assert_eq!(stats.target_frames(), 264);
        assert!(
            out.iter().all(|&v| v == 0.0),
            "played before reaching the target"
        );
        cap.push(&[0.5f32; CHANNELS]);
        ren.render(&mut out);
        assert!(out.iter().all(|&v| v == 0.5));
        assert_eq!(stats.resyncs(), 0);
    }

    #[test]
    fn capture_drops_whole_frames_when_full() {
        let (mut cap, _ren, _s) = channel();
        let block = vec![0.25f32; 1000 * CHANNELS];
        let total: usize = (0..60).map(|_| cap.push(&block)).sum();
        assert_eq!(total, RING_FRAMES);
    }

    #[test]
    fn render_outputs_silence_until_audio_arrives() {
        let (_cap, mut ren, _s) = channel();
        let mut out = vec![1.0f32; 300 * CHANNELS];
        ren.render(&mut out);
        assert!(out.iter().all(|&v| v == 0.0));
    }
}
```

Create `winapp/tests/no_alloc.rs`:
```rust
//! The capture push and render pull (Slip resampler + controller) must not allocate or free.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use stepwave_winapp::drift::{channel, CHANNELS};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = DEALLOCS.try_with(|c| c.set(c.get() + 1));
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn counts() -> (usize, usize) {
    (ALLOCS.with(|c| c.get()), DEALLOCS.with(|c| c.get()))
}

#[test]
fn push_and_render_do_not_allocate() {
    let (mut cap, mut ren, stats) = channel();
    let packet = vec![0.3f32; 480 * CHANNELS];
    let mut out = vec![0.0f32; 441 * CHANNELS];

    // Warm up: first calls may touch lazily initialised state.
    for _ in 0..4 {
        cap.push(&packet);
        ren.render(&mut out);
    }

    let before = counts();
    // Uneven periods (480 in, 441 out) force priming, controller updates and frame slips.
    for i in 0..3000 {
        cap.push(&packet);
        ren.render(&mut out);
        if i % 11 == 0 {
            ren.render(&mut out);
        }
    }
    let after = counts();
    assert_eq!(after.0 - before.0, 0, "render path allocated");
    assert_eq!(after.1 - before.1, 0, "render path freed memory");
    assert!(stats.fill_frames() > 0);
}
```

- [ ] **Step 3: Run to verify failure.** Run `cargo test -p stepwave-winapp`. Expected: compile errors (`cannot find function channel`). The first build also fetches `rubato`, `interprocess` and the Windows-only crates' metadata.

- [ ] **Step 4: Implement.** Put this above the test module in `winapp/src/drift.rs`:

```rust
//! Clock-drift compensation between the capture device (VB-Cable) and the render device.
//!
//! The capture side pushes whole stereo frames into a lock-free ring. The render side pulls
//! fixed chunks through `rubato::Slip`, which occasionally inserts or drops one frame (hidden
//! by a short crossfade) so that the long-run rates match. A PI controller steers the slip
//! ratio from a smoothed ring fill toward a target fill.
//!
//! **Target fill.** Capture arrives in device-period packets and render asks for device-period
//! buffers. Because the two clocks differ, their phases slide through each other; now and then
//! two render periods fall between two capture packets (or the reverse). To never underrun, the
//! ring must hold about two packets plus one chunk. The target therefore adapts to the largest
//! packet or request seen: `2 · max(packet, request) + CHUNK`, at least `MIN_TARGET`. With
//! 10 ms shared-mode periods that is ≈ 21 ms; with 3 ms periods ≈ 7 ms.
//!
//! **Priming and resync.** The render side outputs silence until the ring reaches the target,
//! so playback starts at the target latency. If the ring runs dry (a capture stall) it primes
//! again; if it overfills past `RESYNC_FACTOR · target` (a render stall) it drops the excess.
//! Both count in `resyncs`. The learned clock ratio (the integral term) survives a resync.
//!
//! Real-time safe: `Capture::push` and `Render::render` never allocate, lock or panic.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Adjustable, FixedAsync, Resampler, Slip};

pub const CHANNELS: usize = 2;
/// Render-side processing chunk, in output frames.
pub const CHUNK: usize = 64;
/// Lower bound for the target fill, in frames.
pub const MIN_TARGET: usize = 256;
/// The ring is resynced when it holds more than this many targets.
pub const RESYNC_FACTOR: usize = 4;
/// Largest correction the controller may request, in ppm.
pub const MAX_PPM: f64 = 1000.0;
/// Ring capacity, in frames (1 s).
pub const RING_FRAMES: usize = 48_000;

// Loop design. A correction of `p` ppm changes the fill by `0.048 · p` frames per second, so
// the plant is an integrator with gain 0.048. KP = 2 gives a ~10 s time constant (slow enough
// that the packet sawtooth left after smoothing moves the ratio by only a few ppm); KI removes
// the steady-state offset a constant clock difference would otherwise leave (≈ 30 s).
/// Proportional gain: ppm per frame of fill error.
const KP: f64 = 2.0;
/// Integral gain: ppm per (frame · second) of fill error.
const KI: f64 = 0.07;
/// Per-chunk smoothing of the fill measurement: a ~0.5 s time constant (64-frame chunks)
/// averages out the sawtooth of packet-sized capture pushes.
const FILL_ALPHA: f64 = 0.0027;

/// Shared, lock-free view of the drift state for `status`.
#[derive(Default)]
pub struct DriftStats {
    fill_frames: AtomicU32,
    target_frames: AtomicU32,
    max_packet: AtomicU32,
    ppm_bits: AtomicU64,
    resyncs: AtomicU64,
}

impl DriftStats {
    /// Frames in the ring at the last render chunk.
    pub fn fill_frames(&self) -> u32 {
        self.fill_frames.load(Relaxed)
    }
    /// Current target fill, in frames.
    pub fn target_frames(&self) -> u32 {
        self.target_frames.load(Relaxed)
    }
    /// Current correction, in ppm (negative: the render side consumes less input).
    pub fn ppm(&self) -> f64 {
        f64::from_bits(self.ppm_bits.load(Relaxed))
    }
    /// Times the ring was primed again or trimmed after leaving its safe range.
    pub fn resyncs(&self) -> u64 {
        self.resyncs.load(Relaxed)
    }
}

fn target_for(max_packet: usize) -> usize {
    (2 * max_packet + CHUNK).max(MIN_TARGET)
}

/// Capture end: push interleaved stereo frames.
pub struct Capture {
    tx: rtrb::Producer<f32>,
    stats: Arc<DriftStats>,
}

impl Capture {
    /// Push whole frames; frames that do not fit are dropped whole. Returns frames pushed.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        let frames = (interleaved.len() / CHANNELS) as u32;
        self.stats.max_packet.fetch_max(frames, Relaxed);
        let mut pushed = 0;
        for frame in interleaved.as_chunks::<CHANNELS>().0 {
            if self.tx.slots() < CHANNELS {
                break;
            }
            for &v in frame {
                let _ = self.tx.push(v);
            }
            pushed += 1;
        }
        pushed
    }
}

/// Render end: fill output buffers at the render device's pace.
pub struct Render {
    rx: rtrb::Consumer<f32>,
    slip: Slip<f32>,
    input: Vec<f32>,
    chunk: Vec<f32>,
    chunk_pos: usize,
    target: usize,
    fill_smooth: f64,
    integral: f64,
    priming: bool,
    stats: Arc<DriftStats>,
}

/// Create both ends and the shared stats.
pub fn channel() -> (Capture, Render, Arc<DriftStats>) {
    let (tx, rx) = rtrb::RingBuffer::new(RING_FRAMES * CHANNELS);
    let slip = Slip::<f32>::new(CHUNK, CHANNELS, FixedAsync::Output).expect("valid Slip config");
    let input = vec![0.0; slip.input_frames_max() * CHANNELS];
    let stats = Arc::new(DriftStats::default());
    stats.target_frames.store(MIN_TARGET as u32, Relaxed);
    let render = Render {
        rx,
        slip,
        input,
        chunk: vec![0.0; CHUNK * CHANNELS],
        chunk_pos: CHUNK * CHANNELS, // empty: produce on first call
        target: MIN_TARGET,
        fill_smooth: MIN_TARGET as f64,
        integral: 0.0,
        priming: true,
        stats: stats.clone(),
    };
    let capture = Capture {
        tx,
        stats: stats.clone(),
    };
    (capture, render, stats)
}

impl Render {
    /// Fill `out` (interleaved stereo) completely. Real-time safe.
    pub fn render(&mut self, out: &mut [f32]) {
        let request = (out.len() / CHANNELS) as u32;
        let max_packet = self
            .stats
            .max_packet
            .fetch_max(request, Relaxed)
            .max(request);
        let target = target_for(max_packet as usize);
        if target != self.target {
            self.target = target;
            self.stats.target_frames.store(target as u32, Relaxed);
        }
        let mut written = 0;
        while written < out.len() {
            if self.chunk_pos == self.chunk.len() {
                self.produce_chunk();
            }
            let n = (out.len() - written).min(self.chunk.len() - self.chunk_pos);
            out[written..written + n]
                .copy_from_slice(&self.chunk[self.chunk_pos..self.chunk_pos + n]);
            written += n;
            self.chunk_pos += n;
        }
    }

    fn fill(&self) -> usize {
        self.rx.slots() / CHANNELS
    }

    fn discard(&mut self, frames: usize) {
        for _ in 0..frames * CHANNELS {
            let _ = self.rx.pop();
        }
    }

    fn produce_chunk(&mut self) {
        self.chunk_pos = 0;
        let fill = self.fill();
        if fill > RESYNC_FACTOR * self.target {
            self.discard(fill - self.target);
            self.fill_smooth = self.target as f64;
            self.stats.resyncs.fetch_add(1, Relaxed);
        }
        let fill = self.fill();
        self.stats.fill_frames.store(fill as u32, Relaxed);

        if self.priming {
            if fill < self.target {
                self.chunk.fill(0.0);
                return;
            }
            self.priming = false;
            self.fill_smooth = fill as f64;
        }

        // PI control on the smoothed fill: too full → consume faster (ratio < 1).
        self.fill_smooth += (fill as f64 - self.fill_smooth) * FILL_ALPHA;
        let err = self.fill_smooth - self.target as f64;
        let dt = CHUNK as f64 / 48_000.0;
        let ppm_unclamped = KP * err + KI * (self.integral + err * dt);
        if ppm_unclamped.abs() < MAX_PPM {
            self.integral += err * dt; // anti-windup: only integrate while unsaturated
        }
        let ppm = ppm_unclamped.clamp(-MAX_PPM, MAX_PPM);
        self.stats.ppm_bits.store(ppm.to_bits(), Relaxed);
        let _ = self.slip.set_resample_ratio(1.0 - ppm * 1e-6, false);

        let need = self.slip.input_frames_next();
        if fill < need {
            // Ran dry (capture stalled): silence, and prime again before playing.
            self.chunk.fill(0.0);
            self.priming = true;
            self.stats.resyncs.fetch_add(1, Relaxed);
            return;
        }
        for v in self.input[..need * CHANNELS].iter_mut() {
            *v = self.rx.pop().unwrap_or(0.0);
        }
        let (Ok(inp), Ok(mut outp)) = (
            InterleavedSlice::new(&self.input[..need * CHANNELS], CHANNELS, need),
            InterleavedSlice::new_mut(&mut self.chunk[..], CHANNELS, CHUNK),
        ) else {
            self.chunk.fill(0.0);
            return;
        };
        if self
            .slip
            .process_into_buffer(&inp, &mut outp, None)
            .is_err()
        {
            self.chunk.fill(0.0);
        }
    }

    /// Start over after a device change: drop the backlog to the target, forget the learned
    /// clock ratio (the render clock changed) and prime again.
    pub fn reset(&mut self) {
        let fill = self.fill();
        if fill > self.target {
            self.discard(fill - self.target);
        }
        self.fill_smooth = self.target as f64;
        self.integral = 0.0;
        self.priming = true;
        self.chunk_pos = self.chunk.len();
    }
}
```

- [ ] **Step 5: Verify.**
  1. Run `cargo test -p stepwave-winapp && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 8 drift tests (about 25 s in a debug build, because each covers 10 minutes of audio) and 1 no_alloc test pass.
  2. Commit, push, and confirm the CI `windows` job is green.

- [ ] **Step 6: Commit and push.**

```bash
git add -A
git commit -m "feat(winapp): add drift compensation between capture and render clocks

rubato::Slip on the render side with a PI controller on the smoothed
ring fill; the target adapts to the device period (two periods plus a
chunk) so clock phase slips never underrun; priming and resync on
stalls; lock-free and allocation-free. Adds a windows-latest CI job
(clippy, tests, release build of stepwave.exe as an artifact).

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin m6a-windows
```

---

### Task 3: Control pipe and logon-task installer

**Files:**
- Create: `winapp/src/pipe.rs`, `winapp/src/install.rs`
- Modify: `winapp/src/lib.rs` (add `pub mod install;` and `pub mod pipe;`)
- Test: unit tests in both files (they run on Linux via a Unix socket and on Windows via a named pipe)

**Interfaces:**
- Consumes: `stepwave_host::protocol::{Request, Response, Status, ModeArg}` (Task 1).
- Produces:
  - `pipe::PIPE_NAME = "stepwave"`
  - `pipe::bind(&str) -> io::Result<Listener>`, which returns `AddrInUse` when a live instance answers
  - `pipe::serve(Listener, Fn(Request) -> Response + Send + Sync + 'static) -> JoinHandle<()>`
  - `pipe::request(&str, &Request) -> Result<Response, ClientError>`
  - `ClientError { NotRunning, Io, BadResponse }`
  - `install::{TASK_NAME, task_xml(&Path, &str) -> String, utf16_with_bom(&str) -> Vec<u8>, install(&str) -> Result<()>, uninstall() -> Result<()>}`

- [ ] **Step 1: Write the failing tests.** Add `pub mod install;` and `pub mod pipe;` to `winapp/src/lib.rs`. Create each file containing only its test module:

`winapp/src/pipe.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use stepwave_host::protocol::{ModeArg, Status};

    fn status() -> Status {
        Status {
            enabled: true,
            mode: ModeArg::Auto,
            strength_db: Some(7.0),
            profile: Some("cs2".into()),
            profile_pinned: false,
            processing: "model".into(),
            fallback_reason: None,
            graph_rate: 48_000,
            routed_streams: vec![],
            io: None,
        }
    }

    /// A pipe name unique to this test process and test.
    fn unique(test: &str) -> String {
        format!("stepwave-test-{}-{test}", std::process::id())
    }

    #[test]
    fn round_trip() {
        let pipe = unique("rt");
        serve(bind(&pipe).unwrap(), |req| match req {
            Request::Status => Response::ok(status()),
            _ => Response::err("only status"),
        });
        let r = request(&pipe, &Request::Status).unwrap();
        assert_eq!(r.status.unwrap().processing, "model");
        let r = request(&pipe, &Request::On).unwrap();
        assert_eq!(r.error.as_deref(), Some("only status"));
    }

    #[test]
    fn not_running_and_second_instance() {
        let pipe = unique("nr");
        assert!(matches!(
            request(&pipe, &Request::Status),
            Err(ClientError::NotRunning)
        ));
        serve(bind(&pipe).unwrap(), |_| Response::ok(status()));
        assert_eq!(bind(&pipe).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn silent_client_does_not_block_others() {
        let pipe = unique("sc");
        serve(bind(&pipe).unwrap(), |_| Response::ok(status()));
        let _silent = Stream::connect(name(&pipe).unwrap()).unwrap();
        let start = Instant::now();
        assert!(request(&pipe, &Request::Status).unwrap().ok);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn invalid_and_oversized_requests_get_error_replies() {
        let pipe = unique("bad");
        serve(bind(&pipe).unwrap(), |_| Response::ok(status()));
        for payload in [
            b"{\"cmd\":\"strength\",\"value\":99}\n".to_vec(),
            vec![b'x'; (MAX_LINE + 4096) as usize],
        ] {
            // Full duplex: on Windows a pipe write blocks until the peer reads, so write on
            // one half while reading the reply on the other.
            let conn = Stream::connect(name(&pipe).unwrap()).unwrap();
            let (recv, mut send) = conn.split();
            let writer = std::thread::spawn(move || {
                let _ = send.write_all(&payload);
            });
            let mut reply = String::new();
            BufReader::new(recv).read_line(&mut reply).unwrap();
            let _ = writer.join();
            assert!(reply.contains("\"ok\":false"), "{reply}");
        }
        assert!(request(&pipe, &Request::Status).unwrap().ok);
    }
}
```

`winapp/src/install.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_runs_the_exe_with_run_and_escapes() {
        let xml = task_xml(Path::new(r"C:\Users\A&B\stepwave.exe"), "--mode eq");
        assert!(xml.contains(r"<Command>C:\Users\A&amp;B\stepwave.exe</Command>"));
        assert!(xml.contains("<Arguments>run --mode eq</Arguments>"));
        assert!(xml.contains("<RestartOnFailure><Interval>PT1M</Interval><Count>3</Count>"));
        assert!(xml.contains("<LogonTrigger>"));
        let bare = task_xml(Path::new("s.exe"), "");
        assert!(bare.contains("<Arguments>run</Arguments>"));
    }

    #[test]
    fn utf16_has_bom_and_little_endian_units() {
        assert_eq!(utf16_with_bom("A"), vec![0xFF, 0xFE, 0x41, 0x00]);
    }
}
```

- [ ] **Step 2: Run to verify failure.** Run `cargo test -p stepwave-winapp --lib`. Expected: compile errors (`cannot find function bind`, `task_xml`).

- [ ] **Step 3: Implement.** Put these above the test modules.

`winapp/src/pipe.rs`:
```rust
//! Control channel over a local socket: a named pipe (`\\.\pipe\<name>`) on Windows, an
//! abstract-namespace Unix socket elsewhere (which is what the Linux tests exercise). The
//! protocol is M5's: one JSON request line and one response line per connection.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::{
    prelude::*, GenericNamespaced, Listener, ListenerOptions, Name, Stream,
};
use stepwave_host::protocol::{Request, Response};

/// Pipe name used by `stepwave.exe`.
pub const PIPE_NAME: &str = "stepwave";
/// Longest accepted request or response line, in bytes.
const MAX_LINE: u64 = 64 * 1024;

fn name(pipe: &str) -> io::Result<Name<'_>> {
    pipe.to_ns_name::<GenericNamespaced>()
}

/// Create the listener. If a live instance already answers on `pipe`, fail with `AddrInUse`.
pub fn bind(pipe: &str) -> io::Result<Listener> {
    if Stream::connect(name(pipe)?).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another stepwave instance is running (pipe '{pipe}')"),
        ));
    }
    ListenerOptions::new().name(name(pipe)?).create_sync()
}

/// Serve on a background thread, one thread per connection so a silent client cannot block
/// others. `handler` runs on the connection threads.
pub fn serve<F>(listener: Listener, handler: F) -> std::thread::JoinHandle<()>
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let handler = handler.clone();
            std::thread::spawn(move || {
                if let Err(e) = handle_conn(conn, &*handler) {
                    eprintln!("stepwave: control connection: {e}");
                }
            });
        }
    })
}

fn handle_conn<F: Fn(Request) -> Response>(conn: Stream, handler: &F) -> io::Result<()> {
    // Best effort: Windows named pipes do not support I/O timeouts. A silent client can then
    // only hold its own connection thread, never other requests.
    let _ = conn.set_recv_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(conn.take(MAX_LINE));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response = if !line.ends_with('\n') {
        Response::err(format!(
            "request longer than {MAX_LINE} bytes or not newline-terminated"
        ))
    } else {
        match Request::parse(&line) {
            Ok(req) => handler(req),
            Err(e) => Response::err(e),
        }
    };
    let mut conn = reader.into_inner().into_inner();
    conn.write_all(response.to_line().as_bytes())
}

#[derive(Debug)]
pub enum ClientError {
    /// Nothing is listening on the pipe.
    NotRunning,
    Io(io::Error),
    BadResponse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning => write!(
                f,
                "stepwave is not running; start it with `stepwave run` (or log in again after `stepwave install`)"
            ),
            ClientError::Io(e) => write!(f, "control pipe: {e}"),
            ClientError::BadResponse(e) => write!(f, "bad response from stepwave: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Send one request and wait for the response.
pub fn request(pipe: &str, req: &Request) -> Result<Response, ClientError> {
    let name = name(pipe).map_err(ClientError::Io)?;
    let mut conn = Stream::connect(name).map_err(|_| ClientError::NotRunning)?;
    // Best effort, as on the server side (unsupported on Windows named pipes).
    let _ = conn.set_recv_timeout(Some(Duration::from_secs(10)));
    let mut line = serde_json::to_string(req).expect("Request always serialises");
    line.push('\n');
    conn.write_all(line.as_bytes()).map_err(ClientError::Io)?;
    let mut reply = String::new();
    BufReader::new(conn.take(MAX_LINE))
        .read_line(&mut reply)
        .map_err(ClientError::Io)?;
    if !reply.ends_with('\n') {
        return Err(ClientError::BadResponse("truncated reply".into()));
    }
    serde_json::from_str(reply.trim()).map_err(|e| ClientError::BadResponse(e.to_string()))
}
```

`winapp/src/install.rs`:
```rust
//! `stepwave install | uninstall`: a per-user Task Scheduler task that starts `stepwave run`
//! at logon and restarts it on failure (up to 3 times, a minute apart). No admin rights.

use std::path::Path;

use anyhow::{bail, Context, Result};

/// Task name in Task Scheduler.
pub const TASK_NAME: &str = "stepwave";

/// Task Scheduler XML for `exe run <args>`. Kept platform-neutral so it can be unit-tested.
pub fn task_xml(exe: &Path, args: &str) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>stepwave footstep enhancer</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions>
</Task>
"#,
        escape(&exe.display().to_string()),
        escape(format!("run {args}").trim_end()),
    )
}

/// UTF-16LE with BOM, the encoding `schtasks /XML` expects.
pub fn utf16_with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn schtasks(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("schtasks")
        .args(args)
        .status()
        .context("running schtasks")?;
    if !status.success() {
        bail!("schtasks {} failed ({status})", args.join(" "));
    }
    Ok(())
}

/// Register (or replace) the logon task for the current user.
pub fn install(run_args: &str) -> Result<()> {
    let exe = std::env::current_exe().context("locating stepwave.exe")?;
    let xml = task_xml(&exe, run_args);
    let path = std::env::temp_dir().join("stepwave-task.xml");
    std::fs::write(&path, utf16_with_bom(&xml)).context("writing task XML")?;
    let result = schtasks(&[
        "/Create",
        "/TN",
        TASK_NAME,
        "/XML",
        &path.to_string_lossy(),
        "/F",
    ]);
    let _ = std::fs::remove_file(&path);
    result?;
    println!("installed: stepwave will start at logon (Task Scheduler task '{TASK_NAME}')");
    Ok(())
}

/// Remove the logon task.
pub fn uninstall() -> Result<()> {
    schtasks(&["/Delete", "/TN", TASK_NAME, "/F"])?;
    println!("uninstalled the '{TASK_NAME}' logon task");
    Ok(())
}
```

- [ ] **Step 4: Verify.**
  1. Run `cargo test -p stepwave-winapp && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`. Expected: 4 pipe tests and 2 install tests pass, on top of the drift tests.
  2. Commit, push, and confirm the CI `windows` job is green. This is the named-pipe run.

- [ ] **Step 5: Commit and push.**

```bash
git add -A
git commit -m "feat(winapp): add named-pipe control channel and logon-task installer

The M5 JSON-line protocol over an interprocess local socket (a named
pipe on Windows), one thread per connection, 64 KiB line cap, and
best-effort timeouts (Windows named pipes reject them). install writes
a per-user Task Scheduler logon task with restart-on-failure.

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin m6a-windows
```

---

### Task 4: WASAPI I/O, app wiring and the `stepwave.exe` CLI

**Files:**
- Create: `winapp/src/wasapi_io.rs`, `winapp/src/app.rs`
- Modify: `winapp/src/lib.rs` (final version below), `winapp/src/main.rs` (replace the stub)
- Test: the CI `windows` job (clippy `-D warnings` compiles all `#[cfg(windows)]` code) and a Linux CLI smoke test. Real-device behaviour is checked manually in Task 5.

**Interfaces:**
- Consumes: `drift::{channel, Capture, Render, DriftStats}`, `pipe::{bind, serve, request, PIPE_NAME}`, `install::{install, uninstall}`, `stepwave_host::{audio::channel, engine::{Engine, initial_processor}, protocol::*}`.
- Produces (Windows only):
  - `wasapi_io::{Shared, capture_thread, render_thread, default_render_id, CABLE_CAPTURE}`
  - `app::{Options, run, DEFAULT_PROFILE}`
  - The `stepwave.exe` subcommands `run`, `install`, `uninstall`, `status`, `on`, `off`, `toggle`, `mode`, `strength`, `profile [--auto]` and `reload`, with global flags `--pipe` and `--json`.
- Behaviour:
  - Capture opens the first endpoint whose name contains `CABLE Output`. If none is found, it retries with a 0.25 s → 5 s backoff, and the reason appears in `status` as the note.
  - Render opens the default device and reopens when the main loop sees a new default device id (checked every 1 s), or on error after 1 s.
  - `profile --auto` keeps the current profile and only clears the pin.
  - The main loop ticks the engine every 250 ms.

- [ ] **Step 1: Write the code.**

`winapp/src/lib.rs` (final):
```rust
//! stepwave for Windows: VB-Cable capture → core → WASAPI render, CLI over a named pipe.
//! `drift`, `pipe` and `install`'s XML builder are platform-neutral and tested everywhere;
//! `wasapi_io` and `app` are Windows-only.

pub mod drift;
pub mod install;
pub mod pipe;

#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod wasapi_io;
```

`winapp/src/wasapi_io.rs`:
```rust
//! WASAPI capture (VB-Cable's "CABLE Output") and render (the default output device) loops.
//! Windows only. Each loop runs on its own thread registered with MMCSS "Pro Audio", opens
//! its endpoint in shared, event-driven mode as 48 kHz float stereo (the Windows audio engine
//! converts from the device's own format), and reopens with a backoff when the device goes
//! away. The per-packet paths reuse preallocated buffers: no allocation, locks or logging.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use stepwave_host::audio::AudioCore;
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use crate::drift;

/// Friendly-name fragment that identifies VB-Cable's capture endpoint.
pub const CABLE_CAPTURE: &str = "CABLE Output";
const RATE: usize = 48_000;
const CHANNELS: usize = 2;
const BYTES_PER_FRAME: usize = 4 * CHANNELS;

/// Device state shared with the control side (status reporting and device-change signals).
#[derive(Default)]
pub struct Shared {
    pub stop: AtomicBool,
    /// Set by the capture loop: the open capture endpoint's name.
    pub capture_name: Mutex<Option<String>>,
    /// Set by the render loop: the open render endpoint's name.
    pub render_name: Mutex<Option<String>>,
    /// Why the capture side is not running (e.g. VB-Cable missing), for `status`.
    pub capture_error: Mutex<Option<String>>,
    /// Negotiated capture rate, 0 while closed.
    pub capture_rate: AtomicU32,
    /// Bumped by the main thread when the default render device changes.
    pub render_generation: AtomicU64,
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn format() -> WaveFormat {
    WaveFormat::new(32, 32, &SampleType::Float, RATE, CHANNELS, None)
}

/// Register the calling thread with MMCSS "Pro Audio" (best effort).
fn boost_thread() {
    use windows::core::w;
    use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
    let mut index = 0u32;
    // SAFETY: valid, NUL-terminated task name and a valid out-pointer.
    let _ = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut index) };
}

fn set(slot: &Mutex<Option<String>>, value: Option<String>) {
    if let Ok(mut g) = slot.lock() {
        *g = value;
    }
}

fn find_capture(enumerator: &DeviceEnumerator) -> Res<Option<(wasapi::Device, String)>> {
    let devices = enumerator.get_device_collection(&Direction::Capture)?;
    for i in 0..devices.get_nbr_devices()? {
        let device = devices.get_device_at_index(i)?;
        let name = device.get_friendlyname()?;
        if name.contains(CABLE_CAPTURE) {
            return Ok(Some((device, name)));
        }
    }
    Ok(None)
}

/// Run the capture side until `shared.stop`: VB-Cable → `AudioCore` → drift ring.
pub fn capture_thread(shared: Arc<Shared>, mut core: AudioCore, mut ring: drift::Capture) {
    let _ = wasapi::initialize_mta().ok();
    boost_thread();
    let mut backoff = Duration::from_millis(250);
    while !shared.stop.load(Relaxed) {
        match capture_session(&shared, &mut core, &mut ring) {
            Ok(()) => backoff = Duration::from_millis(250),
            Err(e) => {
                set(&shared.capture_error, Some(e.to_string()));
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        }
        shared.capture_rate.store(0, Relaxed);
        set(&shared.capture_name, None);
    }
}

fn capture_session(shared: &Shared, core: &mut AudioCore, ring: &mut drift::Capture) -> Res<()> {
    let enumerator = DeviceEnumerator::new()?;
    let Some((device, name)) = find_capture(&enumerator)? else {
        return Err(format!("{CABLE_CAPTURE} not found — install VB-Cable").into());
    };
    let mut client = device.get_iaudioclient()?;
    let (default_period, _min) = client.get_device_period()?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: default_period,
    };
    client.initialize_client(&format(), &Direction::Capture, &mode)?;
    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    let frames = client.get_buffer_size()? as usize;
    let mut bytes = vec![0u8; frames * BYTES_PER_FRAME];
    let mut samples = vec![0f32; frames * CHANNELS];
    client.start_stream()?;
    set(&shared.capture_name, Some(name));
    set(&shared.capture_error, None);
    shared.capture_rate.store(RATE as u32, Relaxed);

    while !shared.stop.load(Relaxed) {
        if event.wait_for_event(2000).is_err() {
            // No packets for 2 s: VB-Cable delivers silence while idle, so this means trouble.
            return Err("capture stalled".into());
        }
        loop {
            let packet = capture.get_next_packet_size()?.unwrap_or(0) as usize;
            if packet == 0 {
                break;
            }
            let packet = packet.min(frames);
            let (read, info) = capture.read_from_device(&mut bytes[..packet * BYTES_PER_FRAME])?;
            let n = read as usize * CHANNELS;
            if info.flags.silent {
                samples[..n].fill(0.0);
            } else {
                for (s, b) in samples[..n]
                    .iter_mut()
                    .zip(bytes[..n * 4].as_chunks::<4>().0)
                {
                    *s = f32::from_le_bytes(*b);
                }
            }
            core.process_interleaved(&mut samples[..n]);
            ring.push(&samples[..n]);
        }
    }
    client.stop_stream()?;
    Ok(())
}

/// Run the render side until `shared.stop`: drift ring → default output device. Reopens when
/// the default device changes (`render_generation`) or the device fails.
pub fn render_thread(shared: Arc<Shared>, mut ring: drift::Render) {
    let _ = wasapi::initialize_mta().ok();
    boost_thread();
    while !shared.stop.load(Relaxed) {
        let generation = shared.render_generation.load(Relaxed);
        let result = render_session(&shared, &mut ring, generation);
        set(&shared.render_name, None);
        ring.reset();
        if result.is_err() {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn render_session(shared: &Shared, ring: &mut drift::Render, generation: u64) -> Res<()> {
    let enumerator = DeviceEnumerator::new()?;
    let device = enumerator.get_default_device(&Direction::Render)?;
    let name = device.get_friendlyname()?;
    let mut client = device.get_iaudioclient()?;
    let (default_period, _min) = client.get_device_period()?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: default_period,
    };
    client.initialize_client(&format(), &Direction::Render, &mode)?;
    let event = client.set_get_eventhandle()?;
    let render = client.get_audiorenderclient()?;
    let frames = client.get_buffer_size()? as usize;
    let mut samples = vec![0f32; frames * CHANNELS];
    let mut bytes = vec![0u8; frames * BYTES_PER_FRAME];
    client.start_stream()?;
    set(&shared.render_name, Some(name));

    while !shared.stop.load(Relaxed) && shared.render_generation.load(Relaxed) == generation {
        if event.wait_for_event(1000).is_err() {
            return Err("render stalled".into());
        }
        let space = (client.get_available_space_in_frames()? as usize).min(frames);
        if space == 0 {
            continue;
        }
        let n = space * CHANNELS;
        ring.render(&mut samples[..n]);
        for (b, s) in bytes[..n * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(&samples[..n])
        {
            *b = s.to_le_bytes();
        }
        render.write_to_device(space, &bytes[..n * 4], None)?;
    }
    client.stop_stream()?;
    Ok(())
}

/// Id of the current default render device, for change detection on the main thread.
pub fn default_render_id() -> Option<String> {
    let enumerator = DeviceEnumerator::new().ok()?;
    let device = enumerator.get_default_device(&Direction::Render).ok()?;
    device.get_id().ok()
}
```

`winapp/src/app.rs`:
```rust
//! `stepwave run` on Windows: wires the control pipe, the engine, the capture/render threads
//! and the main loop (engine housekeeping, default-device change detection).

use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use stepwave_host::audio;
use stepwave_host::engine::{initial_processor, Engine};
use stepwave_host::protocol::{IoStatus, ModeArg, Request, Response};

use crate::drift::{self, DriftStats};
use crate::pipe;
use crate::wasapi_io::{self, Shared};

/// Profile used until the user pins another one.
pub const DEFAULT_PROFILE: &str = "cs2";

pub struct Options {
    pub profiles_dir: PathBuf,
    pub mode: ModeArg,
    pub pipe: String,
}

fn io_status(shared: &Shared, drift: &DriftStats) -> IoStatus {
    let name = |m: &Mutex<Option<String>>| m.lock().ok().and_then(|g| g.clone());
    IoStatus {
        capture_device: name(&shared.capture_name),
        render_device: name(&shared.render_name),
        ring_fill_frames: drift.fill_frames(),
        drift_ppm: drift.ppm(),
        resyncs: drift.resyncs(),
    }
}

/// Handle one control request against the shared engine and fill in the Windows fields.
fn handle(engine: &Mutex<Engine>, shared: &Shared, drift: &DriftStats, req: Request) -> Response {
    let Ok(mut engine) = engine.lock() else {
        return Response::err("internal error: engine lock poisoned");
    };
    // `profile --auto` has no game detection to fall back on in M6a: keep the current profile
    // and only clear the pin.
    let keep = matches!(req, Request::Profile(None)).then(|| engine.status(0, &[]).profile);
    let rate = shared.capture_rate.load(Relaxed);
    let mut response = engine.handle(req, rate, &[]);
    if let Some(Some(id)) = keep {
        engine.select_profile(&id);
        response = Response::ok(engine.status(rate, &[]));
    }
    if let Some(status) = response.status.as_mut() {
        status.io = Some(io_status(shared, drift));
        if let Some(err) = shared.capture_error.lock().ok().and_then(|g| g.clone()) {
            status.fallback_reason = Some(match status.fallback_reason.take() {
                Some(r) => format!("{err}; {r}"),
                None => err,
            });
        }
    }
    response
}

pub fn run(opts: Options) -> Result<()> {
    let listener =
        pipe::bind(&opts.pipe).with_context(|| format!("opening control pipe '{}'", opts.pipe))?;

    let (handoff, audio_core) = audio::channel(initial_processor());
    let (mut engine, warnings) = Engine::new(&opts.profiles_dir, opts.mode, handoff);
    for w in warnings {
        eprintln!("stepwave: {w}");
    }
    if let Some(id) = engine.default_profile(DEFAULT_PROFILE) {
        engine.select_profile(&id);
    } else {
        eprintln!(
            "stepwave: no profiles in {}; running in bypass",
            opts.profiles_dir.display()
        );
    }
    let engine = Arc::new(Mutex::new(engine));

    let (capture_ring, render_ring, drift_stats) = drift::channel();
    let shared = Arc::new(Shared::default());

    let (e, s, d) = (engine.clone(), shared.clone(), drift_stats.clone());
    pipe::serve(listener, move |req| handle(&e, &s, &d, req));

    let s = shared.clone();
    let capture = std::thread::Builder::new()
        .name("stepwave-capture".into())
        .spawn(move || wasapi_io::capture_thread(s, audio_core, capture_ring))?;
    let s = shared.clone();
    let render = std::thread::Builder::new()
        .name("stepwave-render".into())
        .spawn(move || wasapi_io::render_thread(s, render_ring))?;

    let _ = wasapi::initialize_mta().ok();
    let mut default_id = wasapi_io::default_render_id();
    let mut last_device_check = Instant::now();
    eprintln!("stepwave: running; pipe '{}'", opts.pipe);
    while !capture.is_finished() && !render.is_finished() {
        std::thread::sleep(Duration::from_millis(250));
        if let Ok(mut e) = engine.lock() {
            e.tick();
        }
        if last_device_check.elapsed() >= Duration::from_secs(1) {
            last_device_check = Instant::now();
            let id = wasapi_io::default_render_id();
            if id != default_id {
                default_id = id;
                shared.render_generation.fetch_add(1, Relaxed);
            }
        }
    }
    shared.stop.store(true, Relaxed);
    anyhow::bail!("an audio thread exited unexpectedly")
}
```

`winapp/src/main.rs`:
```rust
//! `stepwave.exe`: `run` (the long-running app), `install`/`uninstall` (logon task) and the
//! control commands shared with the Linux daemon.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use stepwave_host::protocol::{ModeArg, Request, Response};
use stepwave_winapp::pipe::{self, PIPE_NAME};

#[derive(Parser)]
#[command(
    name = "stepwave",
    version,
    about = "stepwave footstep enhancer for Windows"
)]
struct Cli {
    /// Control pipe name (default: stepwave)
    #[arg(long, global = true, default_value = PIPE_NAME)]
    pipe: String,
    /// Print the raw JSON reply
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run stepwave (normally started at logon by the task `stepwave install` creates)
    Run {
        /// Profiles directory (default: %LOCALAPPDATA%\stepwave\profiles)
        #[arg(long)]
        profiles: Option<PathBuf>,
        /// Initial mode
        #[arg(long, value_enum, default_value = "auto")]
        mode: CliMode,
    },
    /// Start stepwave at logon (per-user Task Scheduler task; no admin rights)
    Install {
        /// Extra arguments for `run`, e.g. "--mode eq"
        #[arg(long, default_value = "")]
        run_args: String,
    },
    /// Remove the logon task
    Uninstall,
    /// Show what stepwave is doing
    Status,
    /// Enable processing
    On,
    /// Bypass processing (latency unchanged, for fair A/B)
    Off,
    /// Flip on/off (bind this to a hotkey)
    Toggle,
    /// Processing mode
    Mode {
        #[arg(value_enum)]
        mode: CliMode,
    },
    /// Override the profile's strength (dB, 0..24) until reload/restart
    Strength { db: f32 },
    /// Pin a profile, or `--auto` to unpin
    Profile {
        #[arg(required_unless_present = "auto")]
        id: Option<String>,
        #[arg(long, conflicts_with = "id")]
        auto: bool,
    },
    /// Re-read profiles and models from disk
    Reload,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CliMode {
    Auto,
    Model,
    Eq,
    Bypass,
}

impl From<CliMode> for ModeArg {
    fn from(m: CliMode) -> Self {
        match m {
            CliMode::Auto => ModeArg::Auto,
            CliMode::Model => ModeArg::Model,
            CliMode::Eq => ModeArg::Eq,
            CliMode::Bypass => ModeArg::Bypass,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let request = match cli.command {
        Command::Run { profiles, mode } => return run(profiles, mode, cli.pipe),
        Command::Install { run_args } => return report(install(&run_args)),
        Command::Uninstall => return report(uninstall()),
        Command::Status => Request::Status,
        Command::On => Request::On,
        Command::Off => Request::Off,
        Command::Toggle => Request::Toggle,
        Command::Mode { mode } => Request::Mode(mode.into()),
        Command::Strength { db } => Request::Strength(db),
        Command::Profile { id, .. } => Request::Profile(id),
        Command::Reload => Request::Reload,
    };
    match pipe::request(&cli.pipe, &request) {
        Ok(resp) => print_response(&resp, cli.json),
        Err(e) => {
            eprintln!("stepwave: {e}");
            ExitCode::FAILURE
        }
    }
}

fn report(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stepwave: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `%LOCALAPPDATA%\stepwave\profiles`.
fn default_profiles_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stepwave")
        .join("profiles")
}

#[cfg(windows)]
fn run(profiles: Option<PathBuf>, mode: CliMode, pipe: String) -> ExitCode {
    let opts = stepwave_winapp::app::Options {
        profiles_dir: profiles.unwrap_or_else(default_profiles_dir),
        mode: mode.into(),
        pipe,
    };
    report(stepwave_winapp::app::run(opts))
}

#[cfg(not(windows))]
fn run(profiles: Option<PathBuf>, _mode: CliMode, _pipe: String) -> ExitCode {
    let dir = profiles.unwrap_or_else(default_profiles_dir);
    eprintln!(
        "stepwave: `run` needs Windows (WASAPI); on Linux use the stepwave daemon ({})",
        dir.display()
    );
    ExitCode::FAILURE
}

#[cfg(windows)]
fn install(run_args: &str) -> anyhow::Result<()> {
    stepwave_winapp::install::install(run_args)
}

#[cfg(not(windows))]
fn install(_run_args: &str) -> anyhow::Result<()> {
    anyhow::bail!("`install` creates a Windows Task Scheduler task and needs Windows")
}

#[cfg(windows)]
fn uninstall() -> anyhow::Result<()> {
    stepwave_winapp::install::uninstall()
}

#[cfg(not(windows))]
fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("`uninstall` needs Windows")
}

fn print_response(resp: &Response, json: bool) -> ExitCode {
    if json {
        print!("{}", resp.to_line());
    } else if let Some(err) = &resp.error {
        eprintln!("stepwave: {err}");
    } else if let Some(status) = &resp.status {
        print!("{}", status.to_text());
    }
    if resp.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
```

- [ ] **Step 2: Verify on Linux.**
  1. Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: clean and green; the Windows modules are not compiled here.
  2. Smoke test the CLI:
     - `cargo run -q -p stepwave-winapp -- status` prints `stepwave: stepwave is not running; …` and exits 1.
     - `cargo run -q -p stepwave-winapp -- run` prints `` `run` needs Windows … `` and exits 1.

- [ ] **Step 3: Verify on Windows (CI).** Commit and push, then wait for the `windows` job. It must pass clippy `-D warnings`, which compiles `wasapi_io` and `app`, pass the tests, and build `stepwave.exe`. If it fails, read the log (`gh run view <id> --log-failed`), fix, push again, and record each round in the report.

- [ ] **Step 4: Commit and push.**

```bash
git add -A
git commit -m "feat(winapp): capture VB-Cable, process with core, render via WASAPI

MMCSS Pro Audio capture and render threads in shared event mode with
engine autoconvert (48 kHz float stereo), capture runs AudioCore then
the drift ring, render pulls through drift compensation; reopen with
backoff when VB-Cable is missing or the default device changes. run
wires engine, pipe and threads; CLI adds run/install/uninstall to the
M5 commands.

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin m6a-windows
```

---

### Task 5: Setup guide, checklist and README

**Files:**
- Create: `docs/windows-setup.md`, `docs/measurements/m6a-checklist.md`
- Modify: `README.md` (intro sentence and a new "Windows app (M6a)" section)
- Test: none (docs only). Check that every command in the guide exists in `winapp/src/main.rs`.

- [ ] **Step 1: Setup guide.** Create `docs/windows-setup.md`:

~~~markdown
# stepwave on Windows (M6a)

stepwave processes **only the game's audio**. CS2 plays into the free VB-Cable virtual device;
`stepwave.exe` captures it, enhances footsteps with `core`, and plays the result on your real
headset or speakers. Discord, browsers and everything else go straight to your device as usual.

> **FACEIT:** do not use stepwave in FACEIT matches until FACEIT Support has confirmed in
> writing that it is allowed. Use it in Premier/Competitive (VAC) first.

## 1. Install VB-Cable

Download "VB-CABLE Driver" from vb-audio.com, run `VBCABLE_Setup_x64.exe` as administrator,
then **reboot**. You now have a playback device **CABLE Input** and a recording device
**CABLE Output**.

## 2. Set formats to 48 kHz

Open the classic Sound Control Panel (`mmsys.cpl`):

- **Playback** tab → *CABLE Input* → Properties → Advanced → **2 channel, 24 bit, 48000 Hz**.
- **Recording** tab → *CABLE Output* → Properties → Advanced → **2 channel, 24 bit, 48000 Hz**.
- **Playback** tab → your headset/speakers → Properties → Advanced → **48000 Hz**.

stepwave asks Windows for 48 kHz float stereo and Windows converts if a device differs, but
matching formats avoids an extra conversion.

## 3. Install stepwave

Build it (`cargo build --release -p stepwave-winapp`) or download `stepwave-windows` from a CI
run, then in PowerShell:

```powershell
$d = "$env:LOCALAPPDATA\stepwave"
New-Item -ItemType Directory -Force "$d\profiles", "$d\models" | Out-Null
Copy-Item stepwave.exe $d
Copy-Item profiles\*.json "$d\profiles"
# Optional: copy trained models if you have them (models\*.swm is not in git).
# Without a model, stepwave uses the profile's static EQ.
Copy-Item models\*.swm "$d\models" -ErrorAction SilentlyContinue
& "$d\stepwave.exe" install     # start at logon (per-user task, no admin)
Start-Process "$d\stepwave.exe" -ArgumentList run -WindowStyle Hidden
& "$d\stepwave.exe" status
```

`status` should show `capture: CABLE Output (VB-Audio Virtual Cable)` and your device under
`render:`.

## 4. Send CS2 to VB-Cable

Start CS2. Open **Settings → System → Sound → Volume mixer**, find **cs2.exe** and set its
**Output device** to **CABLE Input**. Windows remembers this per app.

Now `stepwave status` shows `running: model` (or `eq` if you have no model file) and the
`buffer:` line starts moving.

## 5. A/B hotkey

Create a Desktop shortcut to `"%LOCALAPPDATA%\stepwave\stepwave.exe" toggle`, open its
Properties and set a **Shortcut key** (for example Ctrl+Alt+S). Pressing it flips between
processing and bypass (same latency both ways, so the comparison is fair).

## Everyday commands

```
stepwave status                  # devices, buffer, drift, what is running
stepwave toggle                  # processing <-> bypass
stepwave mode auto|model|eq|bypass
stepwave strength 7              # dB, until reload/restart
stepwave profile cs2             # pin a profile; --auto to unpin
stepwave reload                  # re-read profiles and models
stepwave uninstall               # remove the logon task
```

## If the game goes silent

CS2 plays into VB-Cable, so if stepwave is not running you hear nothing from CS2.

1. `stepwave status`: if it says stepwave is not running, start it:
   `Start-Process "$env:LOCALAPPDATA\stepwave\stepwave.exe" -ArgumentList run -WindowStyle Hidden`.
2. If `note:` says `CABLE Output not found`, reinstall VB-Cable and reboot.
3. Quick escape: in the Volume mixer set cs2.exe's output back to your headset.

## Latency

Total added latency is about 50–60 ms with Windows' default 10 ms device periods: 20 ms of
processing window, about 21 ms of buffer between the two device clocks (two periods plus a
margin, see `buffer:` in `status`), and the two devices' own buffers. That is the price of
processing only the game; bypass has the same latency, so A/B comparisons are fair.
~~~

- [ ] **Step 2: Checklist.** Create `docs/measurements/m6a-checklist.md`:

```markdown
# M6a manual checklist (Windows app)

Run on the Windows PC after following `docs/windows-setup.md`. Premier/Competitive only until
FACEIT confirms. Tick each item and note anything odd.

| # | Check | How | Result |
|---|---|---|---|
| 1 | CS2 is processed, nothing else | `stepwave status` shows `running: model` (or `eq`); Discord and a browser video play normally and are not affected by `toggle` | |
| 2 | `toggle` makes no click | Toggle repeatedly with the hotkey during footsteps | |
| 3 | Headset/speaker switch | Change the default output device mid-game; audio follows within ~1 s | |
| 4 | 30 minutes, no drift | After 30 min of play: `resyncs 0`, `buffer:` near its target, no growing delay | |
| 5 | Perceived latency | Shoot/jump and listen: acceptable for play? Note it | |
| 6 | App stopped | Stop stepwave (Task Manager); CS2 goes silent; start it again and audio returns | |
| 7 | Unplug/replug output | Pull the USB headset out and back in; audio returns | |
| 8 | Missing VB-Cable message | (optional) Disable CABLE Output in `mmsys.cpl`; `status` shows the install hint; re-enable | |
| 9 | Logon start | Reboot; after logging in, `stepwave status` answers without starting it by hand | |
```

- [ ] **Step 3: README.** Make two edits.

(a) In the intro, replace `Runs as a VST3 plugin\nin Equalizer APO (Windows) or a native PipeWire daemon (Linux)` with `Runs as a standalone app\nwith VB-Cable (Windows) or a native PipeWire daemon (Linux)`.

(b) Insert this section directly before `## Training data (M2)`:

```markdown
## Windows app (M6a)

On Windows, CS2 plays into the free VB-Cable virtual device and `stepwave.exe` captures it,
enhances it and plays it on your real device — only the game is processed. Same commands as the
Linux daemon (`stepwave status | toggle | mode | strength | profile | reload`), plus
`stepwave run` and `stepwave install` (start at logon). Setup: [docs/windows-setup.md](docs/windows-setup.md).
```

- [ ] **Step 4: Commit and push.**

```bash
git add docs/windows-setup.md docs/measurements/m6a-checklist.md README.md
git commit -m "docs(winapp): add Windows setup guide, M6a checklist and README section

Step-by-step VB-Cable + stepwave.exe setup (formats, install, Volume
mixer routing, hotkey, recovery), the manual checklist for behaviour
only a real Windows session can show, and the README entry.

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin m6a-windows
```
