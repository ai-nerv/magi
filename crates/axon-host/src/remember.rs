//! What a session should still know tomorrow.
//!
//! Everything a person chose *while using* axon used to be forgotten the moment the daemon
//! stopped. `/model` switched to a model and the next run went back to whatever `axon.model`
//! said; the prompt you typed twenty times was gone. The configuration remembered, and only the
//! configuration — so the only way to keep a choice was to stop making it in the UI and write it
//! in a file instead, which is the opposite of what a UI is for.
//!
//! **Per directory, not global.** A session already records its own `cwd` and
//! [`crate::paths::latest_for`] already filters by it: two projects with different models is the
//! ordinary case, and one global answer would have each project stealing the other's.
//!
//! **A cache, not a configuration.** This lives under the data directory beside the journals,
//! not in `~/.config`. Losing it costs a preference, not a setting, and nothing here is worth
//! hand-editing — `axon.model` is still where a chosen default is *written down*, and this only
//! overrides it for a directory somebody has actually worked in.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// What was chosen, for one directory.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Chosen {
    /// The model, as `axon models` names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How much reasoning to ask for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

/// Where the record lives.
#[must_use]
pub fn path() -> PathBuf {
    crate::paths::sessions_dir()
        .parent()
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("chosen.json")
}

/// What was chosen in `cwd` last time.
///
/// An unreadable or unparseable file is "nothing was chosen". It is a cache: refusing to start
/// over one would be trading a working session for a preference.
#[must_use]
pub fn of(cwd: &str) -> Chosen {
    all().remove(cwd).unwrap_or_default()
}

/// Remember `chosen` for `cwd`, keeping what every other directory chose.
///
/// Read-modify-write rather than held open, because two daemons in two directories are the
/// ordinary case and neither should forget the other. The last writer wins for its own key,
/// which is the only key it touches.
pub fn keep(cwd: &str, chosen: &Chosen) {
    let mut everything = all();
    everything.insert(cwd.to_owned(), chosen.clone());
    let Ok(text) = serde_json::to_string_pretty(&everything) else {
        return;
    };
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

/// Every directory's choices.
fn all() -> BTreeMap<String, Chosen> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_sits_beside_the_journals_rather_than_in_the_config() {
        // Losing it costs a preference, not a setting. `axon.model` is still where a chosen
        // default is written down.
        let path = path();
        assert!(path.ends_with("axon/chosen.json"), "{}", path.display());
        assert!(
            !path.to_string_lossy().contains(".config"),
            "{}",
            path.display()
        );
    }

    #[test]
    fn a_directory_that_chose_nothing_has_chosen_nothing() {
        assert_eq!(of("/no/such/directory/ever"), Chosen::default());
    }

    #[test]
    fn a_choice_is_kept_per_directory() {
        // Two projects with different models is the ordinary case, and one global answer would
        // have each stealing the other's.
        let mut everything: BTreeMap<String, Chosen> = BTreeMap::new();
        everything.insert(
            "/a".to_owned(),
            Chosen {
                model: Some("one".into()),
                thinking: None,
            },
        );
        everything.insert(
            "/b".to_owned(),
            Chosen {
                model: Some("two".into()),
                thinking: Some("high".into()),
            },
        );
        let text = serde_json::to_string(&everything).expect("write");
        let read: BTreeMap<String, Chosen> = serde_json::from_str(&text).expect("read");
        assert_eq!(read["/a"].model.as_deref(), Some("one"));
        assert_eq!(read["/b"].thinking.as_deref(), Some("high"));
        assert_eq!(read["/a"].thinking, None);
    }

    #[test]
    fn a_record_that_will_not_parse_is_nothing_chosen_rather_than_a_failure() {
        // It is a cache. Refusing to start over one would trade a working session for a
        // preference.
        let broken: Result<BTreeMap<String, Chosen>, _> = serde_json::from_str("{ not json");
        assert!(broken.is_err());
        assert_eq!(broken.unwrap_or_default().len(), 0);
    }
}
