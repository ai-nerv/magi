//! The waiting room: what arrives while a turn is running.
//!
//! Split from [`super`] under THE RULE. Nothing another instance says interrupts a turn — a
//! main with ten subagents would otherwise answer the first while the second, third and
//! fourth arrive — so an arrival is held and dealt with when the turn ends.

mod waiting_tests {
    use crate::session::Session;
    use axon_proto::{AgentStatus, Entry, SessionId};

    fn session(name: &str) -> (Session, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("axon-wait-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
        (session, dir)
    }

    fn from(text: &str) -> Entry {
        Entry::From {
            who: "axon/main/beta-nu".to_owned(),
            kin: "main".to_owned(),
            sort: "question".to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_fresh_session_is_idle_and_holding_nothing() {
        let (mut session, dir) = session("fresh");
        assert!(session.idle());
        assert!(session.release().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_mid_turn_is_not_idle() {
        // The whole question an arrival asks. A session that answered "yes" here would take
        // somebody else's message while it was mid-thought about the last one.
        let (mut session, dir) = session("busy");
        session.set_status(AgentStatus::Working {
            label: "Thinking".to_owned(),
        });
        assert!(!session.idle());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_was_held_comes_back_in_the_order_it_arrived() {
        // Ten subagents reporting during one turn is ten things to read, and reading them out
        // of order makes a conversation nobody had.
        let (mut session, dir) = session("order");
        for text in ["first", "second", "third"] {
            session.hold(from(text));
        }
        let out = session.release();
        let said: Vec<&str> = out
            .iter()
            .filter_map(|entry| match entry {
                Entry::From { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(said, ["first", "second", "third"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn releasing_empties_the_room() {
        // Two turns ending close together would otherwise both find the same message there and
        // deal with it twice.
        let (mut session, dir) = session("once");
        session.hold(from("only once"));
        assert_eq!(session.release().len(), 1);
        assert!(session.release().is_empty(), "it came back a second time");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn holding_a_message_does_not_put_it_in_the_transcript() {
        // Not politeness: an entry committed between an assistant's tool call and its result
        // puts a user turn inside an exchange, and no provider accepts that conversation.
        let (mut session, dir) = session("unseen");
        let before = session.entries().len();
        session.hold(from("wait for me"));
        assert_eq!(session.entries().len(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
