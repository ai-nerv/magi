//! What the session knows about its neighbours.
//!
//! Two views of the same handful of fields, kept together because they must agree. One says
//! where this session sits in the tree; the other is the copy the `agent` tool is handed. Built
//! here rather than at each call site so the briefing and the tool cannot end up describing the
//! same neighbour differently — a model told it may stop a session and then refused when it
//! tries has been lied to by the harness, not by the far end.

use super::App;

impl App {
    /// Where this session sits in the tree, for asking whether it may reach somebody.
    #[must_use]
    pub fn whom(&self) -> crate::instance::policy::Whom {
        crate::instance::policy::Whom {
            project: self.identity.project.clone(),
            id: self.identity.id.clone(),
            parent: self.parent.clone(),
        }
    }

    /// Everything the `agent` tool needs in order to answer, copied out of the session.
    ///
    /// A copy rather than a borrow: a tool runs on the turn thread and this is the UI's.
    #[must_use]
    pub fn standing(&self) -> crate::instance::tool::Standing {
        crate::instance::tool::Standing {
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
        let kin = crate::identity::Identity::read(&message.from)
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
            // A question is not answered by being filed, and being called for is not answered by
            // being filed either. Those start a turn; a note waits to be read.
            wake: sort.interrupts() || sort.expects_an_answer(),
        };
        self.inbox.push(message);
        command
    }
}
