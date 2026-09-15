//! What a session should still know tomorrow: the model and effort chosen in the UI, keyed per
//! directory rather than globally. A cache beside the journals, not configuration — losing it costs
//! a preference, and `magi.model` is still where a chosen default is written down.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Chosen {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

#[must_use]
pub fn path() -> PathBuf {
    crate::paths::sessions_dir()
        .parent()
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("chosen.json")
}

/// What was chosen in `cwd` last time. An unreadable or unparseable file is "nothing was chosen".
#[must_use]
pub fn of(cwd: &str) -> Chosen {
    all().remove(cwd).unwrap_or_default()
}

/// Remember `chosen` for `cwd`, keeping what every other directory chose. Read-modify-write, so two
/// daemons in two directories do not forget each other.
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
        let path = path();
        assert!(path.ends_with("magi/chosen.json"), "{}", path.display());
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
        let broken: Result<BTreeMap<String, Chosen>, _> = serde_json::from_str("{ not json");
        assert!(broken.is_err());
        assert_eq!(broken.unwrap_or_default().len(), 0);
    }
}
