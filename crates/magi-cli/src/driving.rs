//! What magi tells its siblings, from its own configuration.
//!
//! One configuration, in one place. A person edits `~/.config/magi/init.lua`; melchior and
//! balthasar are told what follows from it, and neither is left reading a file of its own that
//! might disagree.
//!
//! **Asked before told.** Each sibling declares what it takes and magi answers only that, so a
//! setting one of them renames comes back refused by name instead of failing silently on the far
//! side. What magi has no answer for is simply not sent, and the sibling keeps its own default.

use magi_host::driving;

/// Every sibling magi drives, and what it is called on `PATH`.
const SIBLINGS: &[&str] = &["melchior", "balthasar"];

/// Tell each sibling what this configuration implies for it.
///
/// Quiet when a sibling is not installed: there is nothing to coordinate, and a session that
/// refused to start over an absent sibling would be worse than one that carries on without it.
/// A refusal *is* said out loud — a setting the far side would not take is a coordinator and a
/// sibling disagreeing about what a name means, which is worth a line on stderr.
pub async fn settle(loaded: &crate::config::Loaded) {
    for program in SIBLINGS {
        let needs = driving::needs(program).await;
        if needs.is_empty() {
            continue;
        }
        let said = answers(loaded, program);
        let borrowed: Vec<(&str, serde_json::Value)> =
            said.iter().map(|(k, v)| (&**k, v.clone())).collect();
        let source = driving::saying(program, &needs, &borrowed);
        if source.trim().is_empty() {
            continue;
        }
        match driving::configure(program, &source).await {
            Ok(applied) => {
                for refused in applied.refused {
                    eprintln!(
                        "magi: {program} would not take {}: {}",
                        refused.name, refused.why
                    );
                }
            }
            Err(why) => eprintln!("magi: {program} could not be configured: {why}"),
        }
    }
}

/// What magi's configuration says, in the vocabulary each sibling uses.
///
/// Two sources, and the sibling's own block wins.
///
/// ```lua
/// magi.model    = "openrouter/anthropic/claude-sonnet-4.5"  -- shared: what magi is using
/// magi.thinking = "off"
///
/// magi.balthasar = { promote_floor = 0.6 }   -- a sibling's own vocabulary
/// magi.melchior  = { max_tokens = 4000 }
/// ```
///
/// The shared pair are settings magi genuinely has an opinion about and a sibling might share the
/// name for. Everything else belongs to one sibling and is written under its name, so a person
/// reading the config can see which program a line is aimed at — and a name that sibling does not
/// take comes back refused rather than sitting there doing nothing.
fn answers(loaded: &crate::config::Loaded, program: &str) -> Vec<(String, serde_json::Value)> {
    let mut out: Vec<(String, serde_json::Value)> = Vec::new();
    if let Some(thinking) = loaded.config.string("thinking") {
        out.push(("thinking".to_owned(), serde_json::Value::from(thinking)));
    }
    if let Some(model) = loaded.config.string("model") {
        out.push(("model".to_owned(), serde_json::Value::from(model)));
    }
    // Last, so a sibling's own block overrides the shared answer for it.
    if let Some(table) = loaded.config.get(program).and_then(|v| v.as_object()) {
        for (name, value) in table {
            out.retain(|(held, _)| held != name);
            out.push((name.clone(), value.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_lua::Engine;

    pub(super) fn loaded(source: &str) -> crate::config::Loaded {
        let mut engine = Engine::new();
        engine.run(source, "test").expect("config");
        engine.harvest();
        crate::config::Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
        }
    }

    #[test]
    fn what_the_configuration_says_is_what_is_offered() {
        let held = loaded(
            r#"
            magi.thinking = "high"
            magi.model = "openrouter/x"
            "#,
        );
        let offered = answers(&held, "melchior");
        assert!(offered.iter().any(|(name, _)| *name == "thinking"));
        assert!(offered.iter().any(|(name, _)| *name == "model"));
    }

    #[test]
    fn a_setting_magi_has_no_opinion_about_is_not_invented() {
        // A coordinator repeating a sibling's own default is a coordinator that will drift from
        // it the first time the sibling changes its mind.
        let offered = answers(&loaded(""), "melchior");
        assert!(offered.is_empty(), "{offered:?}");
    }

    #[tokio::test]
    async fn an_absent_sibling_is_passed_over_rather_than_fatal() {
        // Nothing installed under these names in a test environment is the ordinary case; this
        // must return rather than refuse.
        settle(&loaded(r#"magi.thinking = "off""#)).await;
    }
}

#[cfg(test)]
mod blocks {
    use super::tests::loaded;
    use super::*;

    #[test]
    fn a_siblings_own_block_is_offered_to_it_and_to_nobody_else() {
        let held = loaded(
            r#"
            magi.balthasar = { promote_floor = 0.6 }
            magi.melchior  = { max_tokens = 4000 }
            "#,
        );
        let to_balthasar = answers(&held, "balthasar");
        assert!(to_balthasar.iter().any(|(n, _)| n == "promote_floor"));
        assert!(!to_balthasar.iter().any(|(n, _)| n == "max_tokens"));

        let to_melchior = answers(&held, "melchior");
        assert!(to_melchior.iter().any(|(n, _)| n == "max_tokens"));
        assert!(!to_melchior.iter().any(|(n, _)| n == "promote_floor"));
    }

    #[test]
    fn a_siblings_block_wins_over_the_shared_answer() {
        // Both name `thinking`. The one written under the sibling is the one aimed at it, so it
        // is the one that goes -- and it goes once, not twice with the last write deciding.
        let held = loaded(
            r#"
            magi.thinking = "off"
            magi.melchior = { thinking = "high" }
            "#,
        );
        let said = answers(&held, "melchior");
        let thinking: Vec<_> = said.iter().filter(|(n, _)| n == "thinking").collect();
        assert_eq!(thinking.len(), 1, "said twice: {said:?}");
        assert_eq!(thinking[0].1, serde_json::json!("high"));
    }
}
