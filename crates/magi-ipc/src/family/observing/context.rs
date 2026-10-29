#[derive(Clone, Debug, serde::Serialize)]
pub struct Context {
    schema: u8,
    run_id: String,
    agent_id: String,
    turn_id: Option<String>,
    ask_id: Option<String>,
}

impl Context {
    pub fn new(
        run: String,
        agent: String,
        turn: Option<String>,
        ask: Option<String>,
    ) -> Option<Self> {
        let valid = |id: &str| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if !valid(&run)
            || !valid(&agent)
            || turn.as_deref().is_some_and(|id| !valid(id))
            || ask.as_deref().is_some_and(|id| !valid(id))
        {
            return None;
        }
        Some(Self {
            schema: 1,
            run_id: run,
            agent_id: agent,
            turn_id: turn,
            ask_id: ask,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_has_only_bounded_canonical_identities() {
        let id = "0123456789abcdef0123456789abcdef";
        let context =
            Context::new(id.into(), id.into(), Some(id.into()), Some(id.into())).expect("context");
        let body = serde_json::to_vec(&context).expect("encoded context");
        assert!(body.len() <= 256);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).expect("json")["schema"],
            1
        );
        for invalid in [
            "",
            "private-prompt",
            "0123456789ABCDEF0123456789ABCDEF",
            &"0".repeat(33),
        ] {
            assert!(Context::new(invalid.into(), id.into(), None, None).is_none());
            assert!(Context::new(id.into(), invalid.into(), None, None).is_none());
            assert!(Context::new(id.into(), id.into(), Some(invalid.into()), None).is_none());
            assert!(Context::new(id.into(), id.into(), None, Some(invalid.into())).is_none());
        }
    }
}
