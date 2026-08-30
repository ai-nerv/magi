//! Reading one setting at a time.
//!
//! Split from the loading under THE RULE. This module does two jobs -- put the Lua files
//! together in the right order, then answer questions about what they said -- and the second
//! half grows every time a setting is added.

use super::Loaded;

/// Permissions the configuration granted outright.
///
/// `axon.allow` is a list of rules somebody wrote down deliberately, which is a question already
/// answered: those go into the ledger at startup rather than being prompted for. Anything not
/// listed is asked about the first time it comes up.
///
/// ```lua
/// axon.allow = {
///   { verb = "read",  anything = true },
///   { verb = "run",   program = "git" },
///   { verb = "write", directory = "/home/you/work" },
/// }
/// ```
#[must_use]
pub fn grants(loaded: &Loaded) -> Vec<axon_proto::permit::Grant> {
    use axon_proto::permit::{Grant, Scope};
    let Some(rules) = loaded.config.get("allow").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    rules
        .iter()
        .filter_map(|rule| {
            let verb = rule.get("verb")?.as_str()?.to_owned();
            let scope = if rule.get("anything").and_then(serde_json::Value::as_bool) == Some(true) {
                Scope::Anything
            } else if let Some(program) = rule.get("program").and_then(|v| v.as_str()) {
                Scope::Program {
                    program: program.to_owned(),
                }
            } else if let Some(path) = rule.get("directory").and_then(|v| v.as_str()) {
                Scope::Directory {
                    path: path.to_owned(),
                }
            } else {
                // A rule naming no width grants nothing. Silently widening a typo to `Anything`
                // would be the worst possible reading of it.
                return None;
            };
            Some(Grant { verb, scope })
        })
        .collect()
}

/// What the model is told it is, for this session.
///
/// Assembled here because this is where the configuration and the working directory are both
/// in hand. Every milestone before this one sent nothing: the model got tool schemas and no
/// idea what it was, where it was, or what machine it was on.
#[must_use]
pub fn system(loaded: &Loaded) -> Option<String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    axon_host::system::assemble(loaded.config.string("system"), &cwd, &today())
}

/// Today, as the model should read it.
fn today() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Civil from days, so a date needs no calendar crate. The model wants to know roughly
    // when it is, not to do arithmetic with it.
    let days = i64::try_from(seconds / 86_400).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// What to ask for beyond the conversation.
///
/// `axon.thinking` is off unless asked for. Reasoning costs tokens and money, and a default
/// that quietly spends both is the wrong kind of surprise — but the whole branch that requests
/// it existed in every protocol description with nothing ever setting this, so asking was
/// impossible rather than merely off.
#[must_use]
pub fn options(loaded: &Loaded) -> axon_provider::api::Options {
    let thinking = loaded
        .config
        .string("thinking")
        .and_then(|level| serde_json::from_value(serde_json::Value::String(level.to_owned())).ok());
    axon_provider::api::Options {
        // Set per request, not per session: a schema belongs to one question.
        schema: None,
        thinking,
        max_tokens: None,
    }
}

/// Everything `axon.ui` says about how the screen looks.
///
/// One table, three kinds of value, and the names come from the three modules themselves — so a
/// colour, a glyph or a size that exists is one a config can set, and there is no list here to
/// keep in step with them.
///
/// ```lua
/// axon.ui.accent    = 1
/// axon.ui.marker    = "▶ "
/// axon.ui.menu_rows = 12
/// ```
///
/// A name that is not any of theirs is ignored rather than refused: a config written for a later
/// axon should not stop an earlier one from starting.
pub fn adopt_ui(loaded: &Loaded) {
    let Some(ui) = loaded.config.get("ui").and_then(|v| v.as_object()) else {
        return;
    };

    // A value of the wrong kind is left alone rather than coerced. `accent = "red"` is a mistake,
    // and painting something anyway would hide it behind a colour nobody chose.
    let mut palette = axon_tui::colour::Palette::default();
    palette.overlay(&|name| {
        ui.get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u8::try_from(n).ok())
    });
    axon_tui::colour::adopt(palette);

    let mut glyphs = axon_tui::glyph::Glyphs::default();
    glyphs.overlay(&|name| {
        ui.get(name)
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    });
    // A spinner with no frames is a division by zero at the one moment somebody is watching, so
    // an empty list is read as "say nothing about the spinner" rather than obeyed.
    if let Some(frames) = ui.get("spinner").and_then(|v| v.as_array()) {
        let drawn: Vec<String> = frames
            .iter()
            .filter_map(|f| f.as_str().map(ToOwned::to_owned))
            .collect();
        if !drawn.is_empty() {
            glyphs.spinner = drawn;
        }
    }
    axon_tui::glyph::adopt(glyphs);

    let mut metrics = axon_tui::metric::Metrics::default();
    metrics.overlay(&|name| ui.get(name).and_then(serde_json::Value::as_u64));
    axon_tui::metric::adopt(metrics);
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    pub(super) fn from_lua(source: &str) -> Loaded {
        let mut engine = axon_lua::Engine::new();
        engine.run(source, "test").expect("the config must run");
        engine.harvest();
        Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
            apis: Vec::new(),
            providers: Vec::new(),
        }
    }

    /// The palette a config would produce, without adopting it process-wide.
    fn palette_of(source: &str) -> axon_tui::colour::Palette {
        let loaded = from_lua(source);
        let ui = loaded
            .config
            .get("ui")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut palette = axon_tui::colour::Palette::default();
        palette.overlay(&|name| {
            ui.get(name)
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u8::try_from(n).ok())
        });
        palette
    }

    #[test]
    fn a_config_that_says_nothing_gets_the_ordinary_terminal() {
        assert_eq!(palette_of(""), axon_tui::colour::STOCK);
    }

    #[test]
    fn a_field_can_be_set_without_declaring_the_table_first() {
        // `axon.ui` exists before any config runs, so this is an assignment rather than an
        // attempt to index a nil.
        let chosen = palette_of("axon.ui.accent = 1");
        assert_eq!(chosen.accent, 1);
        assert_eq!(chosen.muted, axon_tui::colour::STOCK.muted, "and only that");
    }

    #[test]
    fn the_whole_table_can_be_replaced_at_once() {
        let chosen = palette_of("axon.ui = { accent = 1, muted = 8, border = 237 }");
        assert_eq!((chosen.accent, chosen.muted, chosen.border), (1, 8, 237));
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_left_alone() {
        // Painting something anyway would hide the mistake behind a colour nobody chose.
        let chosen = palette_of("axon.ui.accent = 300\naxon.ui.dim = 'grey'");
        assert_eq!(chosen.accent, axon_tui::colour::STOCK.accent);
        assert_eq!(chosen.dim, axon_tui::colour::STOCK.dim);
    }

    #[test]
    fn a_name_nothing_recognises_is_ignored_rather_than_refused() {
        // A config written for a later axon still starts this one.
        let loaded = from_lua("axon.ui.from_the_future = 3");
        assert!(loaded.config.get("ui").is_some(), "it ran");
    }

    #[test]
    fn the_three_kinds_are_named_apart() {
        // One flat table only works if a colour, a glyph and a size never want the same name.
        use axon_tui::{colour::Palette, glyph::Glyphs, metric::Metrics};
        let mut all: Vec<&str> = Vec::new();
        all.extend(Palette::NAMES);
        all.extend(Glyphs::NAMES);
        all.extend(Metrics::NAMES);
        let mut seen = std::collections::BTreeSet::new();
        for name in all {
            assert!(seen.insert(name), "{name} is claimed twice");
        }
    }
}

/// Environment every process axon starts is given, beside the mandatory pairs.
///
/// ```lua
/// axon.env = { RUST_LOG = "warn", PAGER = "cat" }
/// ```
///
/// A flat table of strings. Anything that is not one is skipped rather than stringified: an
/// environment variable holding `table: 0x...` is a typo that would otherwise reach a shell.
#[must_use]
pub fn environ(loaded: &Loaded) -> std::collections::BTreeMap<String, String> {
    let Some(table) = loaded.config.get("env").and_then(|v| v.as_object()) else {
        return std::collections::BTreeMap::new();
    };
    table
        .iter()
        .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
        .collect()
}

#[cfg(test)]
mod environ_tests {
    use super::*;
    use axon_lua::Engine;

    fn from(source: &str) -> std::collections::BTreeMap<String, String> {
        let mut engine = Engine::new();
        engine.run(source, "test").expect("config");
        engine.harvest();
        environ(&Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
            apis: Vec::new(),
            providers: Vec::new(),
        })
    }

    #[test]
    fn a_config_that_says_nothing_adds_nothing() {
        assert!(from("").is_empty());
    }

    #[test]
    fn a_table_of_strings_comes_through() {
        let seen = from(r#"axon.env = { PAGER = "cat", RUST_LOG = "warn" }"#);
        assert_eq!(seen.get("PAGER").map(String::as_str), Some("cat"));
        assert_eq!(seen.get("RUST_LOG").map(String::as_str), Some("warn"));
    }

    #[test]
    fn a_value_that_is_not_a_string_is_left_out() {
        // Otherwise a nested table reaches a shell as `table: 0x55f...`, which is a typo that
        // presents as a mysterious environment rather than as a mistake in the config.
        let seen = from(r#"axon.env = { GOOD = "yes", BAD = { 1, 2 } }"#);
        assert_eq!(seen.get("GOOD").map(String::as_str), Some("yes"));
        assert!(!seen.contains_key("BAD"));
    }
}

/// How long a daemon stays up with nobody attached and nothing running.
///
/// ```lua
/// axon.idle_exit = 600   -- seconds; 0 keeps it up forever
/// ```
///
/// Ten minutes by default. Detaching is not ending a session — a UI that quits mid-turn should
/// be able to come back to the answer — so this is a grace period, not a hangup. Without one, a
/// daemon outlives every session that ever opened it and they accumulate one per directory.
#[must_use]
pub fn idle_exit(loaded: &Loaded) -> std::time::Duration {
    let seconds = loaded.config.number("idle_exit").unwrap_or(600.0);
    if seconds <= 0.0 {
        return std::time::Duration::ZERO;
    }
    std::time::Duration::from_secs_f64(seconds)
}

/// The opening scramble is off unless a config asks for it.
#[cfg(test)]
mod decrypt_tests {
    use super::ui_tests::from_lua;
    use super::*;

    /// What `axon.ui` says, as the overlay would read it.
    fn ui_of(loaded: &Loaded) -> serde_json::Value {
        loaded
            .config
            .get("ui")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    }

    #[test]
    fn it_is_off_until_somebody_asks() {
        // The whole point of an opt-in: a config that says nothing about it pays nothing for it.
        assert_eq!(axon_tui::metric::BUILT_IN.decrypt_ms, 0);
    }

    #[test]
    fn a_config_can_switch_it_on() {
        let loaded = from_lua("axon.ui.decrypt_ms = 900");
        let ui = ui_of(&loaded);
        let mut metrics = axon_tui::metric::Metrics::default();
        metrics.overlay(&|name| ui.get(name).and_then(serde_json::Value::as_u64));
        assert_eq!(metrics.decrypt_ms, 900);
    }

    #[test]
    fn and_choose_what_it_scrambles_with() {
        let loaded = from_lua(r#"axon.ui.decrypt_pool = "01""#);
        let ui = ui_of(&loaded);
        let mut glyphs = axon_tui::glyph::Glyphs::default();
        glyphs.overlay(&|name| {
            ui.get(name)
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        });
        assert_eq!(glyphs.decrypt_pool, "01");
    }
}
