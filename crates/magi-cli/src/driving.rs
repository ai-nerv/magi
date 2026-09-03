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
        let source = driving::saying(program, &needs, &answers(loaded));
        if source.trim().is_empty() {
            continue;
        }
        match driving::configure(program, &source).await {
            Ok(applied) => {
                for refused in applied.refused {
                    eprintln!("magi: {program} would not take {}: {}", refused.name, refused.why);
                }
            }
            Err(why) => eprintln!("magi: {program} could not be configured: {why}"),
        }
    }
}

/// What magi's configuration says, in the vocabulary a sibling might share.
///
/// Names are the sibling's, not magi's: `thinking` means the same thing on both sides, and a
/// sibling that does not take one simply never sees it. Anything magi has no opinion about is
/// left out entirely rather than sent as a default, because a coordinator repeating a sibling's
/// own default is a coordinator that will drift from it.
fn answers(loaded: &crate::config::Loaded) -> Vec<(&'static str, serde_json::Value)> {
    let mut out: Vec<(&'static str, serde_json::Value)> = Vec::new();
    if let Some(thinking) = loaded.config.string("thinking") {
        out.push(("thinking", serde_json::Value::from(thinking)));
    }
    if let Some(model) = loaded.config.string("model") {
        out.push(("model", serde_json::Value::from(model)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_lua::Engine;

    fn loaded(source: &str) -> crate::config::Loaded {
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
        let offered = answers(&held);
        assert!(offered.iter().any(|(name, _)| *name == "thinking"));
        assert!(offered.iter().any(|(name, _)| *name == "model"));
    }

    #[test]
    fn a_setting_magi_has_no_opinion_about_is_not_invented() {
        // A coordinator repeating a sibling's own default is a coordinator that will drift from
        // it the first time the sibling changes its mind.
        let offered = answers(&loaded(""));
        assert!(offered.is_empty(), "{offered:?}");
    }

    #[tokio::test]
    async fn an_absent_sibling_is_passed_over_rather_than_fatal() {
        // Nothing installed under these names in a test environment is the ordinary case; this
        // must return rather than refuse.
        settle(&loaded(r#"magi.thinking = "off""#)).await;
    }
}
