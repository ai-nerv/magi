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
}
