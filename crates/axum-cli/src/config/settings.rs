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
