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
