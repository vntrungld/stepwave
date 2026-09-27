//! Per-game profile (`profiles/*.json`).

use serde::Deserialize;

use crate::CoreError;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(rename = "match")]
    pub match_rules: MatchRules,
    /// Model file; unused until M4.
    pub model: String,
    /// Footstep boost strength; unused until M4.
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
}
