//! Choosing and building a `Processor` for a profile: shared by the CLI and the daemon.
//! Runs off the audio thread (it allocates and reads model files).

use std::path::{Path, PathBuf};

use crate::mask::UnityMask;
use crate::stft::SAMPLE_RATE;
use crate::swm::SwmModel;
use crate::{CoreError, Processor, Profile};

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Model if it loads, else static EQ.
    Auto,
    /// Model; see [`build`] for what happens when it cannot load.
    Model,
    /// The profile's static EQ.
    Eq,
    /// Unity gain (same latency as the other modes).
    Bypass,
}

/// What is actually running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Processing {
    Model,
    Eq,
    Bypass,
}

impl Processing {
    pub fn as_str(self) -> &'static str {
        match self {
            Processing::Model => "model",
            Processing::Eq => "eq",
            Processing::Bypass => "bypass",
        }
    }
}

pub struct Built {
    pub processor: Processor,
    pub processing: Processing,
    /// Why the result is not what `mode` asked for (e.g. the model failed to load).
    pub fallback_reason: Option<String>,
}

/// A profile's `model` path: absolute as-is, else relative to the parent of the directory
/// holding the profile (`<root>/profiles/x.json` → `<root>/<model>`).
pub fn resolve_model_path(profile: &Profile, profile_path: &Path) -> PathBuf {
    let model = Path::new(&profile.model);
    if model.is_absolute() {
        return model.to_path_buf();
    }
    let root = profile_path
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."));
    root.join(model)
}

/// Build a processor. `Auto` falls back to static EQ when the model cannot load. `Model`
/// is strict when `strict_model` is true (the offline CLI) and falls back like `Auto`
/// otherwise (the daemon, which must never go silent).
pub fn build(
    mode: Mode,
    profile: &Profile,
    model_path: &Path,
    strict_model: bool,
) -> Result<Built, CoreError> {
    let eq = |reason: Option<String>| -> Result<Built, CoreError> {
        Ok(Built {
            processor: Processor::new(profile, SAMPLE_RATE)?,
            processing: Processing::Eq,
            fallback_reason: reason,
        })
    };
    match mode {
        Mode::Bypass => Ok(Built {
            processor: Processor::with_mask(Box::new(UnityMask), SAMPLE_RATE)?,
            processing: Processing::Bypass,
            fallback_reason: None,
        }),
        Mode::Eq => eq(None),
        Mode::Model | Mode::Auto => match SwmModel::load(model_path) {
            Ok(model) => Ok(Built {
                processor: Processor::with_model(profile, &model, SAMPLE_RATE)?,
                processing: Processing::Model,
                fallback_reason: None,
            }),
            Err(err) if mode == Mode::Model && strict_model => Err(err),
            Err(err) => eq(Some(format!(
                "model {} unusable ({err}); using static EQ",
                model_path.display()
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CS2: &str = include_str!("../../profiles/cs2.json");
    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/model_random.swm"
    );

    fn profile() -> Profile {
        Profile::from_json(CS2).unwrap()
    }

    #[test]
    fn model_path_is_relative_to_profiles_parent() {
        let p = profile();
        assert_eq!(
            resolve_model_path(&p, Path::new("/home/u/.config/stepwave/profiles/cs2.json")),
            Path::new("/home/u/.config/stepwave/models/cs2.swm")
        );
    }

    #[test]
    fn absolute_model_path_is_kept() {
        let mut p = profile();
        p.model = "/opt/m.swm".into();
        assert_eq!(
            resolve_model_path(&p, Path::new("/x/profiles/cs2.json")),
            Path::new("/opt/m.swm")
        );
    }

    #[test]
    fn each_mode_builds_what_it_says() {
        let p = profile();
        let ok = Path::new(FIXTURE);
        assert_eq!(
            build(Mode::Model, &p, ok, true).unwrap().processing,
            Processing::Model
        );
        assert_eq!(
            build(Mode::Auto, &p, ok, true).unwrap().processing,
            Processing::Model
        );
        assert_eq!(
            build(Mode::Eq, &p, ok, true).unwrap().processing,
            Processing::Eq
        );
        assert_eq!(
            build(Mode::Bypass, &p, ok, true).unwrap().processing,
            Processing::Bypass
        );
    }

    #[test]
    fn missing_model_falls_back_except_strict_model() {
        let p = profile();
        let missing = Path::new("/nonexistent/m.swm");
        let auto = build(Mode::Auto, &p, missing, true).unwrap();
        assert_eq!(auto.processing, Processing::Eq);
        assert!(auto.fallback_reason.unwrap().contains("/nonexistent/m.swm"));
        let lenient = build(Mode::Model, &p, missing, false).unwrap();
        assert_eq!(lenient.processing, Processing::Eq);
        assert!(lenient.fallback_reason.is_some());
        assert!(build(Mode::Model, &p, missing, true).is_err());
    }
}
