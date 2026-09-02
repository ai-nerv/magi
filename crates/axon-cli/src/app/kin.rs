//! A message from another session, on its way into the conversation.
//!
//! atom is where one lands — it holds the socket — and the session is where it belongs, because
//! the transcript and the turns are the session's. So this is the seam: what came up atom's pipe
//! becomes a command for the session, and nothing here decides anything about it.
//!
//! The sort arrives already chosen, by the sender, and is not second-guessed. A second opinion
//! about what a message *is* would be a second chance to get it wrong.

use super::App;

impl App {
    /// A message arrived from another session.
    ///
    /// Handed to the session rather than pushed onto the transcript here. An entry the UI kept
    /// for itself was one the model never saw, so an instance could be asked a question and sit
    /// there until somebody typed at it.
    pub fn received(&mut self, who: &str, sort: &str, text: &str) -> axon_proto::UiCommand {
        // Counted now, and only ever a count. What is *unanswered* is the one thing a sibling
        // asking `status` cares about, and the one thing atom cannot work out for itself: it
        // cannot see a turn end.
        self.waiting += 1;
        axon_proto::UiCommand::Arrived {
            who: who.to_owned(),
            kin: relation(who, &self.named),
            sort: sort.to_owned(),
            text: text.to_owned(),
        }
    }

    /// How many arrivals are still waiting on an answer.
    #[must_use]
    pub fn unanswered(&self) -> usize {
        self.waiting
    }

    /// A turn ended, so whatever it was answering is answered.
    pub fn answered(&mut self) {
        self.waiting = 0;
    }
}

/// How the sender stands to us, from the two names.
///
/// A thin reading, deliberately. The *authority* is atom, which holds the directory and the
/// notes beside each socket; what is here is only enough to label a block on screen — whether
/// the sender is this project's, and whether it is us. Anything finer is asked of atom by the
/// tool that needs it, rather than guessed at twice.
fn relation(who: &str, me: &str) -> String {
    let project = |name: &str| name.split('/').next().unwrap_or_default().to_owned();
    if who == me {
        return "myself".to_owned();
    }
    if project(who) != project(me) {
        return "elsewhere".to_owned();
    }
    "main".to_owned()
}

/// An arrival becomes a command, and is counted once.
#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.named = "axon/main/alpha-rho".to_owned();
        app
    }

    #[test]
    fn an_arrival_goes_to_the_session_rather_than_onto_the_screen() {
        // The transcript and the turns are the session's. An entry the UI kept for itself was
        // one the model never saw, so an instance could be asked a question and sit there.
        let mut app = app();
        let command = app.received("axon/main/beta-nu", "attention", "the parser is done");
        let axon_proto::UiCommand::Arrived {
            who, sort, text, ..
        } = command
        else {
            panic!("an arrival is an arrival");
        };
        assert_eq!(who, "axon/main/beta-nu");
        assert_eq!(sort, "attention");
        assert_eq!(text, "the parser is done");
    }

    #[test]
    fn what_is_waiting_is_what_has_not_been_answered() {
        // What a sibling asking `status` is really asking, and the one part of an inbox the
        // layer holding it cannot work out on its own.
        let mut app = app();
        assert_eq!(app.unanswered(), 0);
        app.received("axon/main/beta-nu", "question", "which parser?");
        app.received("axon/main/gamma-xi", "attention", "look at this");
        assert_eq!(app.unanswered(), 2);
        app.answered();
        assert_eq!(app.unanswered(), 0, "a turn answered them");
    }

    #[test]
    fn a_sender_in_another_project_is_marked_as_being_elsewhere() {
        // It should never happen — atom does not reach across projects — so one that arrives
        // should say so on the block rather than quietly reading as a neighbour.
        let mut app = app();
        let command = app.received("elsewhere/main/beta-nu", "note", "hello");
        let axon_proto::UiCommand::Arrived { kin, .. } = command else {
            panic!("an arrival");
        };
        assert_eq!(kin, "elsewhere");
    }

    #[test]
    fn a_session_talking_to_itself_is_labelled_as_itself() {
        let mut app = app();
        let command = app.received("axon/main/alpha-rho", "note", "a note to self");
        let axon_proto::UiCommand::Arrived { kin, .. } = command else {
            panic!("an arrival");
        };
        assert_eq!(kin, "myself");
    }
}

impl App {
    /// Another session is asking to become this one's child.
    ///
    /// Put to the person, not to the model. Accepting means this session's authority is lent to
    /// another — what the child may then do is what this session may do — and a model deciding
    /// that on its own behalf would be granting itself a second pair of hands.
    ///
    /// The same picker a permission uses, deliberately: it is the same kind of question, asked of
    /// the same person, and a second modal shape for it would be one more thing to recognise
    /// under pressure. Where the answer *goes* is what differs, and that is
    /// [`Picking::Adoption`](super::Picking::Adoption)'s job.
    pub fn asked_to_adopt(&mut self, id: &str, who: &str, why: &str) {
        let them = who.rsplit('/').next().unwrap_or(who);
        // Their words, and marked as theirs. A reason printed bare reads as though the session
        // were saying it, and this is the one part of the question somebody else wrote.
        let detail = if why.trim().is_empty() {
            "no reason given".to_owned()
        } else {
            format!("“{}”", why.trim())
        };
        self.overlay = Some(
            axon_tui::picker::Picker::new(
                format!("`{them}` asks to work under this session — {detail}"),
                vec![
                    axon_tui::picker::Choice {
                        value: "yes".to_owned(),
                        detail: "it becomes this session's child, and may do what this may"
                            .to_owned(),
                        ready: true,
                    },
                    axon_tui::picker::Choice {
                        value: "no".to_owned(),
                        detail: "refuse, and tell them so".to_owned(),
                        ready: true,
                    },
                ],
                None,
            )
            .into(),
        );
        self.picking = Some(super::Picking::Adoption { id: id.to_owned() });
    }
}

/// Being asked to take a session on is put to the person, and answered either way.
#[cfg(test)]
mod adopting {

    /// The heading the person actually reads.
    fn titled(app: &App) -> String {
        match app.overlay.as_ref() {
            Some(axon_tui::overlay::Overlay::Picker(picker)) => picker.title.clone(),
            _ => String::new(),
        }
    }
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.named = "axum/main/alpha-rho".to_owned();
        app
    }

    #[test]
    fn the_question_takes_the_screen_and_says_who_is_asking() {
        let mut app = app();
        app.asked_to_adopt("r1", "axum/main/beta-nu", "I am running the migration");
        let Some(crate::app::Picking::Adoption { id }) = app.picking.as_ref() else {
            panic!("the request must be remembered, or the answer has nothing to name");
        };
        assert_eq!(id, "r1");
        assert!(app.overlay.is_some(), "nothing was put in front of anybody");
    }

    #[test]
    fn their_reason_is_shown_as_theirs() {
        // The one part of the question somebody else wrote. Printed bare it reads as though
        // this session were saying it, and a person deciding whether to lend their authority
        // should see whose words they are weighing.
        let mut app = app();
        app.asked_to_adopt("r1", "axum/main/beta-nu", "I am running the migration");
        let shown = titled(&app);
        assert!(shown.contains("beta-nu"), "{shown}");
        assert!(shown.contains("running the migration"), "{shown}");
    }

    #[test]
    fn a_request_with_no_reason_still_says_so() {
        // Rather than an empty pair of quotes, which reads as a rendering fault.
        let mut app = app();
        app.asked_to_adopt("r1", "axum/main/beta-nu", "   ");
        assert!(titled(&app).contains("no reason given"), "{}", titled(&app));
    }

    #[test]
    fn accepting_is_not_something_a_model_can_reach() {
        // The whole reason this goes up the pipe rather than into the transcript. A model that
        // could accept on its own behalf would be granting itself a second pair of hands, and
        // the only thing that answers this is a keypress.
        let mut app = app();
        app.asked_to_adopt("r1", "axum/main/beta-nu", "why");
        // An arrival is what a model sees, and it does not touch the pending request.
        let _ = app.received("axum/main/gamma-xi", "note", "unrelated");
        assert!(
            matches!(app.picking, Some(crate::app::Picking::Adoption { .. })),
            "a message changed a question only a person may answer"
        );
    }
}
