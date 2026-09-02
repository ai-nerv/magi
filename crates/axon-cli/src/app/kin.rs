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
