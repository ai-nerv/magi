//! What the session knows about its neighbours.
//!
//! Two views of the same handful of fields, kept together because they must agree. One says
//! where this session sits in the tree; the other is the copy the `agent` tool is handed. Built
//! here rather than at each call site so the briefing and the tool cannot end up describing the
//! same neighbour differently — a model told it may stop a session and then refused when it
//! tries has been lied to by the harness, not by the far end.

use super::App;

impl App {
    /// Everything the `agent` tool needs in order to answer, copied out of the session.
    ///
    /// A copy rather than a borrow: a tool runs on the turn thread and this is the UI's.
    #[must_use]
    pub fn standing(&self) -> axon_agent::verbs::Standing {
        axon_agent::verbs::Standing {
            me: self.identity.full(),
            parent: self.parent.clone(),
            forked: self.forked.clone(),
            minted: self.minted.clone(),
            inbox: self.inbox.clone(),
        }
    }

    /// A message arrived from another axon.
    ///
    /// Two places, and both are needed. The transcript is so somebody *sees* it — a message that
    /// only reached a queue is a message nobody knew about until they thought to ask, which for
    /// an `attention` is the whole of the failure. The inbox is so the model can act on it: it
    /// is what the `agent` tool reads, and reading a message is not the same as answering it.
    pub fn received(&mut self, message: crate::instance::wire::Message) -> axon_proto::UiCommand {
        let kin = crate::instance::Identity::read(&message.from)
            .map_or(crate::instance::policy::Relation::Elsewhere, |who| {
                self.standing().stands(&who)
            });
        let sort = message.sort;
        let command = axon_proto::UiCommand::Arrived {
            who: message.from.clone(),
            // Stamped now rather than looked up when it is drawn: it was true when the message
            // arrived, and a session that has since forked would redraw the whole transcript
            // with relations that did not hold at the time.
            kin: kin.word().to_owned(),
            sort: serde_json::to_value(sort)
                .ok()
                .and_then(|sort| sort.as_str().map(ToOwned::to_owned))
                .unwrap_or_default(),
            text: message.text.clone(),
        };
        self.inbox.push(message);
        command
    }
}

/// A message reaches the session, and the sorts agree about which of them wants an answer.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::wire::{Message, Sort};

    fn app() -> App {
        let mut app = App::new();
        app.identity = crate::instance::Identity {
            project: "axon".to_owned(),
            role: "main".to_owned(),
            id: "alpha-rho".to_owned(),
        };
        app
    }

    fn arriving(sort: Sort) -> axon_proto::Entry {
        let command = app().received(Message::sent("axon/main/beta-nu", "hello", sort, None));
        let axon_proto::UiCommand::Arrived {
            who,
            kin,
            sort,
            text,
        } = command
        else {
            panic!("an arrival is an arrival");
        };
        axon_proto::Entry::From {
            who,
            kin,
            sort,
            text,
        }
    }

    #[test]
    fn an_arrival_goes_to_the_session_rather_than_onto_the_screen() {
        // The transcript and the turns are the session's. An entry the UI kept for itself was
        // one the model never saw, so an instance could be asked a question and sit there.
        let mut app = app();
        let command = app.received(Message::new("axon/main/beta-nu", "the parser is done"));
        assert!(matches!(command, axon_proto::UiCommand::Arrived { .. }));
        assert_eq!(app.inbox.len(), 1, "and the tool can still read it");
    }

    #[test]
    fn the_two_halves_agree_about_which_sorts_want_an_answer() {
        // The rule lives in the session — `axon_host::wants_answering` — and the sorts live
        // here. Nothing would fail if they drifted: a sort added on this side would simply stop
        // waking anybody, quietly, in a way no test that only knew one half could see.
        for sort in [
            Sort::Note,
            Sort::Question,
            Sort::Answer,
            Sort::Attention,
            Sort::Claim,
            Sort::Release,
            Sort::Handoff,
            Sort::Trouble,
        ] {
            let ours = sort.interrupts() || sort.expects_an_answer();
            let theirs = axon_host::wants_answering(&arriving(sort));
            assert_eq!(ours, theirs, "{sort:?}");
        }
    }

    #[test]
    fn a_note_is_read_rather_than_answered() {
        assert!(!axon_host::wants_answering(&arriving(Sort::Note)));
        assert!(!axon_host::wants_answering(&arriving(Sort::Answer)));
    }

    #[test]
    fn being_asked_or_called_for_is_answered() {
        for sort in [Sort::Question, Sort::Attention, Sort::Trouble] {
            assert!(axon_host::wants_answering(&arriving(sort)), "{sort:?}");
        }
    }
}
