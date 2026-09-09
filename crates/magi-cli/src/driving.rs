//! What magi tells its siblings, from its own configuration. Each sibling declares what it takes and
//! magi answers only that, so a setting one of them renames comes back refused by name instead of
//! failing silently. What magi has no answer for is not sent, and the sibling keeps its own default.

use magi_host::driving;

/// Every sibling magi drives, and what it is called on `PATH`.
const SIBLINGS: &[&str] = &["casper", "melchior", "balthasar"];

/// Tell each sibling what this configuration implies for it. Quiet when a sibling is not installed;
/// a refusal is said out loud, being a coordinator and a sibling disagreeing about what a name means.
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

/// What magi's configuration says, in the vocabulary each sibling uses. Two sources, and the
/// sibling's own block wins:
///
/// ```lua
/// magi.model    = "openrouter/anthropic/claude-sonnet-4.5"  -- shared: what magi is using
/// magi.thinking = "off"
///
/// magi.balthasar = { promote_floor = 0.6 }   -- a sibling's own vocabulary
/// magi.melchior  = { max_tokens = 4000 }
/// ```
///
/// A name the sibling does not take comes back refused rather than sitting there doing nothing.
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
        // A coordinator repeating a sibling's own default will drift from it.
        let offered = answers(&loaded(""), "melchior");
        assert!(offered.is_empty(), "{offered:?}");
    }

    #[tokio::test]
    async fn an_absent_sibling_is_passed_over_rather_than_fatal() {
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
    fn caspers_table_of_tools_is_aimed_at_casper() {
        // The one sibling whose settings are tables rather than scalars.
        let held = loaded(r#"magi.casper = { tools = { dino = { off = true } } }"#);
        let said = answers(&held, "casper");
        assert!(said.iter().any(|(n, _)| n == "tools"), "{said:?}");
        assert!(!answers(&held, "melchior").iter().any(|(n, _)| n == "tools"));
        assert!(SIBLINGS.contains(&"casper"), "and it is actually driven");
    }

    #[test]
    fn a_siblings_block_wins_over_the_shared_answer() {
        // Both name `thinking`. The one written under the sibling goes, once.
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
