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
        }
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
