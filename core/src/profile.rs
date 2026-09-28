//! Per-game profile (`profiles/*.json`).

use serde::Deserialize;

use crate::CoreError;

/// Largest `|gain|`, in dB, allowed for `preamp_db` or any `fallback_eq` band.
/// A huge configured gain would silently blow past the limiter's headroom
/// intent and defeats the point of validating the profile at all.
pub const MAX_GAIN_DB: f32 = 24.0;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(rename = "match")]
    pub match_rules: MatchRules,
    /// Model file, relative to the repo root.
    pub model: String,
    /// Footstep boost, in dB; model gains scale by `strength_db / model
    /// strength_db` (0 = unity, i.e. the model is bypassed to 0 dB gain).
    pub strength_db: f32,
    #[serde(default)]
    pub fallback_eq: Vec<EqBand>,
    #[serde(default)]
    pub preamp_db: f32,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct MatchRules {
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub linux: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EqKind {
    Lowshelf,
    Highshelf,
    Peak,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct EqBand {
    #[serde(rename = "type")]
    pub kind: EqKind,
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
}

impl Profile {
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        let profile: Profile = serde_json::from_str(json)?;
        profile.validate()?;
        Ok(profile)
    }

    fn validate(&self) -> Result<(), CoreError> {
        if !self.preamp_db.is_finite() || !self.strength_db.is_finite() {
            return Err(CoreError::InvalidProfile(
                "preamp_db and strength_db must be finite".into(),
            ));
        }
        if self.preamp_db.abs() > MAX_GAIN_DB {
            return Err(CoreError::InvalidProfile(format!(
                "preamp_db {} must have |gain| <= {MAX_GAIN_DB} dB",
                self.preamp_db
            )));
        }
        if !(0.0..=MAX_GAIN_DB).contains(&self.strength_db) {
            return Err(CoreError::InvalidProfile(format!(
                "strength_db {} must be in [0, {MAX_GAIN_DB}] dB",
                self.strength_db
            )));
        }
        for (i, band) in self.fallback_eq.iter().enumerate() {
            if !(band.freq > 0.0 && band.freq < 24_000.0) {
                return Err(CoreError::InvalidProfile(format!(
                    "fallback_eq[{i}]: freq {} Hz must be in (0, 24000)",
                    band.freq
                )));
            }
            if !(band.q > 0.0 && band.q.is_finite()) {
                return Err(CoreError::InvalidProfile(format!(
                    "fallback_eq[{i}]: q {} must be > 0",
                    band.q
                )));
            }
            if !band.gain.is_finite() {
                return Err(CoreError::InvalidProfile(format!(
                    "fallback_eq[{i}]: gain must be finite"
                )));
            }
            if band.gain.abs() > MAX_GAIN_DB {
                return Err(CoreError::InvalidProfile(format!(
                    "fallback_eq[{i}]: gain {} must have |gain| <= {MAX_GAIN_DB} dB",
                    band.gain
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CS2: &str = include_str!("../../profiles/cs2.json");

    #[test]
    fn parses_cs2_profile() {
        let p = Profile::from_json(CS2).unwrap();
        assert_eq!(p.id, "cs2");
        assert_eq!(p.match_rules.windows, vec!["cs2.exe"]);
        assert_eq!(p.preamp_db, -4.0);
        assert_eq!(p.fallback_eq.len(), 3);
        assert_eq!(
            p.fallback_eq[0],
            EqBand {
                kind: EqKind::Lowshelf,
                freq: 120.0,
                gain: -4.0,
                q: 0.7
            }
        );
    }

    fn with_band(band: &str) -> String {
        format!(
            r#"{{"id":"t","name":"T","match":{{}},"model":"m.swm","strength_db":6.0,
                "fallback_eq":[{band}],"preamp_db":0.0}}"#
        )
    }

    #[test]
    fn rejects_non_positive_q() {
        let json = with_band(r#"{"type":"peak","freq":1000,"gain":3,"q":0}"#);
        assert!(matches!(
            Profile::from_json(&json),
            Err(CoreError::InvalidProfile(_))
        ));
    }

    #[test]
    fn rejects_frequency_at_or_above_nyquist() {
        let json = with_band(r#"{"type":"peak","freq":24000,"gain":3,"q":1}"#);
        assert!(matches!(
            Profile::from_json(&json),
            Err(CoreError::InvalidProfile(_))
        ));
    }

    #[test]
    fn rejects_unknown_filter_type() {
        let json = with_band(r#"{"type":"notch","freq":1000,"gain":3,"q":1}"#);
        assert!(matches!(Profile::from_json(&json), Err(CoreError::Json(_))));
    }

    #[test]
    fn missing_eq_and_preamp_default_to_empty_and_zero() {
        let json = r#"{"id":"t","name":"T","match":{},"model":"m.swm","strength_db":6.0}"#;
        let p = Profile::from_json(json).unwrap();
        assert!(p.fallback_eq.is_empty());
        assert_eq!(p.preamp_db, 0.0);
    }

    #[test]
    fn rejects_excessive_band_gain() {
        let json = with_band(r#"{"type":"peak","freq":1000,"gain":30,"q":1}"#);
        assert!(matches!(
            Profile::from_json(&json),
            Err(CoreError::InvalidProfile(_))
        ));
    }

    #[test]
    fn rejects_excessive_preamp() {
        let json = r#"{"id":"t","name":"T","match":{},"model":"m.swm","strength_db":6.0,
            "preamp_db":-30.0}"#;
        assert!(matches!(
            Profile::from_json(json),
            Err(CoreError::InvalidProfile(_))
        ));
    }

    #[test]
    fn cs2_profile_still_parses_with_gain_limit_enforced() {
        assert!(Profile::from_json(CS2).is_ok());
    }

    #[test]
    fn profile_rejects_strength_out_of_range() {
        let cs2: serde_json::Value =
            serde_json::from_str(include_str!("../../profiles/cs2.json")).unwrap();
        let with_strength = |s: f64| {
            let mut v = cs2.clone();
            v["strength_db"] = s.into();
            v.to_string()
        };
        for bad in [-1.0, 30.0] {
            assert!(
                Profile::from_json(&with_strength(bad)).is_err(),
                "strength {bad} accepted"
            );
        }
        assert!(Profile::from_json(&with_strength(0.0)).is_ok());
    }
}
