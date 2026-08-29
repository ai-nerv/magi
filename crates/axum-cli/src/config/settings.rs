//! Reading one setting at a time.
//!
//! Split from the loading under THE RULE. This module does two jobs -- put the Lua files
//! together in the right order, then answer questions about what they said -- and the second
//! half grows every time a setting is added.

use super::Loaded;

/// Permissions the configuration granted outright.
///
/// `axum.allow` is a list of rules somebody wrote down deliberately, which is a question already
/// answered: those go into the ledger at startup rather than being prompted for. Anything not
/// listed is asked about the first time it comes up.
///
/// ```lua
/// axum.allow = {
///   { verb = "read",  anything = true },
///   { verb = "run",   program = "git" },
///   { verb = "write", directory = "/home/you/work" },
/// }
/// ```
#[must_use]
pub fn grants(loaded: &Loaded) -> Vec<axum_proto::permit::Grant> {
    use axum_proto::permit::{Grant, Scope};
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
    axum_host::system::assemble(loaded.config.string("system"), &cwd, &today())
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
/// `axum.thinking` is off unless asked for. Reasoning costs tokens and money, and a default
/// that quietly spends both is the wrong kind of surprise — but the whole branch that requests
/// it existed in every protocol description with nothing ever setting this, so asking was
/// impossible rather than merely off.
#[must_use]
pub fn options(loaded: &Loaded) -> axum_provider::api::Options {
    let thinking = loaded
        .config
        .string("thinking")
        .and_then(|level| serde_json::from_value(serde_json::Value::String(level.to_owned())).ok());
    axum_provider::api::Options {
        // Set per request, not per session: a schema belongs to one question.
        schema: None,
        thinking,
        max_tokens: None,
    }
}

/// Everything `axum.ui` says about how the screen looks.
///
/// One table, three kinds of value, and the names come from the three modules themselves — so a
/// colour, a glyph or a size that exists is one a config can set, and there is no list here to
/// keep in step with them.
///
/// ```lua
/// axum.ui.accent    = 1
/// axum.ui.marker    = "▶ "
/// axum.ui.menu_rows = 12
/// ```
///
/// A name that is not any of theirs is ignored rather than refused: a config written for a later
/// axum should not stop an earlier one from starting.
pub fn adopt_ui(loaded: &Loaded) {
    let Some(ui) = loaded.config.get("ui").and_then(|v| v.as_object()) else {
        return;
    };

    // A value of the wrong kind is left alone rather than coerced. `accent = "red"` is a mistake,
    // and painting something anyway would hide it behind a colour nobody chose.
    let mut palette = axum_tui::colour::Palette::default();
    palette.overlay(&|name| {
        ui.get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u8::try_from(n).ok())
    });
    axum_tui::colour::adopt(palette);

    let mut glyphs = axum_tui::glyph::Glyphs::default();
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
    axum_tui::glyph::adopt(glyphs);

    let mut metrics = axum_tui::metric::Metrics::default();
    metrics.overlay(&|name| ui.get(name).and_then(serde_json::Value::as_u64));
    axum_tui::metric::adopt(metrics);
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    fn from_lua(source: &str) -> Loaded {
        let mut engine = axum_lua::Engine::new();
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
    fn palette_of(source: &str) -> axum_tui::colour::Palette {
        let loaded = from_lua(source);
        let ui = loaded
            .config
            .get("ui")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut palette = axum_tui::colour::Palette::default();
        palette.overlay(&|name| {
            ui.get(name)
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u8::try_from(n).ok())
        });
        palette
    }

    #[test]
    fn a_config_that_says_nothing_gets_the_ordinary_terminal() {
        assert_eq!(palette_of(""), axum_tui::colour::STOCK);
    }

    #[test]
    fn a_field_can_be_set_without_declaring_the_table_first() {
        // `axum.ui` exists before any config runs, so this is an assignment rather than an
        // attempt to index a nil.
        let chosen = palette_of("axum.ui.accent = 1");
        assert_eq!(chosen.accent, 1);
        assert_eq!(chosen.muted, axum_tui::colour::STOCK.muted, "and only that");
    }

    #[test]
    fn the_whole_table_can_be_replaced_at_once() {
        let chosen = palette_of("axum.ui = { accent = 1, muted = 8, border = 237 }");
        assert_eq!((chosen.accent, chosen.muted, chosen.border), (1, 8, 237));
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_left_alone() {
        // Painting something anyway would hide the mistake behind a colour nobody chose.
        let chosen = palette_of("axum.ui.accent = 300\naxum.ui.dim = 'grey'");
        assert_eq!(chosen.accent, axum_tui::colour::STOCK.accent);
        assert_eq!(chosen.dim, axum_tui::colour::STOCK.dim);
    }

    #[test]
    fn a_name_nothing_recognises_is_ignored_rather_than_refused() {
        // A config written for a later axum still starts this one.
        let loaded = from_lua("axum.ui.from_the_future = 3");
        assert!(loaded.config.get("ui").is_some(), "it ran");
    }

    #[test]
    fn the_three_kinds_are_named_apart() {
        // One flat table only works if a colour, a glyph and a size never want the same name.
        use axum_tui::{colour::Palette, glyph::Glyphs, metric::Metrics};
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
