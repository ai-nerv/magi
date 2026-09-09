//! The waiting room: what arrives while a turn is running. Nothing another instance says
//! interrupts a turn — an arrival is held and dealt with when the turn ends.

mod waiting_tests {
    use crate::session::Session;
    use magi_model::scratch::Scratch;
    use magi_proto::{AgentStatus, Entry, SessionId};

    fn session(name: &str) -> (Session, Scratch) {
        let dir = Scratch::new("magi-wait", name);
        let session = Session::recorded(SessionId::new("s"), Vec::new());
        (session, dir)
    }

    fn from(text: &str) -> Entry {
        Entry::From {
            who: "magi/main/beta-nu".to_owned(),
            kin: "main".to_owned(),
            sort: "question".to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_fresh_session_is_idle_and_holding_nothing() {
        let (mut session, _dir) = session("fresh");
        assert!(session.idle());
        assert!(session.release().is_empty());
    }

    #[test]
    fn a_session_mid_turn_is_not_idle() {
        let (mut session, _dir) = session("busy");
        session.set_status(AgentStatus::Working {
            label: "Thinking".to_owned(),
        });
        assert!(!session.idle());
    }

    #[test]
    fn what_was_held_comes_back_in_the_order_it_arrived() {
        let (mut session, _dir) = session("order");
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
    }

    #[test]
    fn releasing_empties_the_room() {
        let (mut session, _dir) = session("once");
        session.hold(from("only once"));
        assert_eq!(session.release().len(), 1);
        assert!(session.release().is_empty(), "it came back a second time");
    }

    #[test]
    fn holding_a_message_does_not_put_it_in_the_transcript() {
        // An entry committed between an assistant's tool call and its result puts a user turn
        // inside an exchange, and no provider accepts that conversation.
        let (mut session, _dir) = session("unseen");
        let before = session.entries().len();
        session.hold(from("wait for me"));
        assert_eq!(session.entries().len(), before);
    }
}

/// Which arrivals make an idle session think, and which only make it better informed.
#[cfg(test)]
mod waking {
    use magi_proto::Entry;

    fn arrived(sort: &str) -> Entry {
        Entry::From {
            who: "magi/main/beta-nu".to_owned(),
            kin: "main".to_owned(),
            sort: sort.to_owned(),
            text: "…".to_owned(),
        }
    }

    #[test]
    fn an_answer_to_a_question_this_session_asked_starts_a_turn() {
        // `reply` used not to wake the asker, so every conversation was one exchange long.
        assert!(
            crate::wants_answering(&arrived("answer")),
            "a reply must resume the session that asked, or `ask` is a one-way trip"
        );
    }

    #[test]
    fn work_handed_over_starts_a_turn() {
        // `handoff` moves work rather than copying it: not waking for it stops the work outright.
        assert!(crate::wants_answering(&arrived("handoff")));
    }

    #[test]
    fn being_asked_or_called_on_starts_a_turn() {
        for sort in ["question", "attention", "trouble"] {
            assert!(crate::wants_answering(&arrived(sort)), "{sort}");
        }
    }

    #[test]
    fn a_note_is_read_by_the_time_you_next_answer_rather_than_now() {
        // A session that starts a turn for everything that arrives is one nobody leaves running.
        for sort in ["note", "claim", "release"] {
            assert!(!crate::wants_answering(&arrived(sort)), "{sort}");
        }
    }

    #[test]
    fn a_sort_from_a_newer_layer_does_not_start_a_turn_by_accident() {
        // An unknown sort is committed and read like a note; the programs are released apart.
        assert!(!crate::wants_answering(&arrived("whistling")));
    }

    #[test]
    fn nothing_that_is_not_a_message_wakes_anybody() {
        assert!(!crate::wants_answering(&Entry::Notice {
            text: "…".to_owned(),
        }));
    }
}
