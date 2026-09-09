//! A message from another session, on its way into the conversation: what came up melchior's pipe
//! becomes a command for the session. The sort arrives already chosen by the sender.

use super::App;

impl App {
    /// A message arrived from another session. Handed to the session rather than pushed onto the
    /// transcript here, or the model would never see it.
    pub fn received(&mut self, who: &str, sort: &str, text: &str) -> magi_proto::UiCommand {
        // melchior cannot work this out for itself: it cannot see a turn end.
        self.waiting += 1;
        magi_proto::UiCommand::Arrived {
            who: who.to_owned(),
            kin: relation(who, &self.named),
            sort: sort.to_owned(),
            text: text.to_owned(),
        }
    }

    #[must_use]
    pub fn unanswered(&self) -> usize {
        self.waiting
    }

    pub fn answered(&mut self) {
        self.waiting = 0;
    }
}

/// How the sender stands to us, from the two names. Only enough to label a block on screen.
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
        app.named = "magi/main/alpha-rho".to_owned();
        app
    }

    #[test]
    fn an_arrival_goes_to_the_session_rather_than_onto_the_screen() {
        let mut app = app();
        let command = app.received("magi/main/beta-nu", "attention", "the parser is done");
        let magi_proto::UiCommand::Arrived {
            who, sort, text, ..
        } = command
        else {
            panic!("an arrival is an arrival");
        };
        assert_eq!(who, "magi/main/beta-nu");
        assert_eq!(sort, "attention");
        assert_eq!(text, "the parser is done");
    }

    #[test]
    fn what_is_waiting_is_what_has_not_been_answered() {
        let mut app = app();
        assert_eq!(app.unanswered(), 0);
        app.received("magi/main/beta-nu", "question", "which parser?");
        app.received("magi/main/gamma-xi", "attention", "look at this");
        assert_eq!(app.unanswered(), 2);
        app.answered();
        assert_eq!(app.unanswered(), 0, "a turn answered them");
    }

    #[test]
    fn a_sender_in_another_project_is_marked_as_being_elsewhere() {
        // melchior does not reach across projects, so one that arrives must not read as a neighbour.
        let mut app = app();
        let command = app.received("elsewhere/main/beta-nu", "note", "hello");
        let magi_proto::UiCommand::Arrived { kin, .. } = command else {
            panic!("an arrival");
        };
        assert_eq!(kin, "elsewhere");
    }

    #[test]
    fn a_session_talking_to_itself_is_labelled_as_itself() {
        let mut app = app();
        let command = app.received("magi/main/alpha-rho", "note", "a note to self");
        let magi_proto::UiCommand::Arrived { kin, .. } = command else {
            panic!("an arrival");
        };
        assert_eq!(kin, "myself");
    }
}

impl App {
    /// Another session is asking to become this one's child. Put to the person, not the model, since
    /// accepting lends this session's authority away. Answered by [`Picking::Adoption`](super::Picking::Adoption).
    pub fn asked_to_adopt(&mut self, id: &str, who: &str, why: &str) {
        let them = who.rsplit('/').next().unwrap_or(who);
        // Quoted, because this is the one part of the question somebody else wrote.
        let detail = if why.trim().is_empty() {
            "no reason given".to_owned()
        } else {
            format!("“{}”", why.trim())
        };
        self.overlay = Some(
            magi_tui::picker::Picker::new(
                format!("`{them}` asks to work under this session — {detail}"),
                vec![
                    magi_tui::picker::Choice {
                        value: "yes".to_owned(),
                        detail: "it becomes this session's child, and may do what this may"
                            .to_owned(),
                        ready: true,
                    },
                    magi_tui::picker::Choice {
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

    /// What this session may do, for lending to a child. A copy taken at the moment of accepting.
    #[must_use]
    pub fn lending(&self) -> Vec<magi_proto::permit::Grant> {
        self.granted.clone()
    }

    /// Remember a grant this session was given, so a child can be lent it later. The ledger that
    /// enforces it is on the worker thread and never read back.
    pub fn was_granted(&mut self, grant: magi_proto::permit::Grant) {
        if !self.granted.contains(&grant) {
            self.granted.push(grant);
        }
    }
}

/// Being asked to take a session on is put to the person, and answered either way.
#[cfg(test)]
mod adopting {

    fn titled(app: &App) -> String {
        match app.overlay.as_ref() {
            Some(magi_tui::overlay::Overlay::Picker(picker)) => picker.title.clone(),
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
        // Printed bare it reads as though this session were saying it.
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
        // A model that could accept on its own behalf would be granting itself a second pair of hands.
        let mut app = app();
        app.asked_to_adopt("r1", "axum/main/beta-nu", "why");
        let _ = app.received("axum/main/gamma-xi", "note", "unrelated");
        assert!(
            matches!(app.picking, Some(crate::app::Picking::Adoption { .. })),
            "a message changed a question only a person may answer"
        );
    }
}

/// What a session lends the one it takes on.
#[cfg(test)]
mod lending_grants {
    use super::*;
    use magi_proto::permit::{Action, Grant, Scope};

    fn app() -> App {
        let mut app = App::new();
        app.named = "axum/main/alpha-rho".to_owned();
        app
    }

    fn run(program: &str) -> Grant {
        Grant {
            verb: "run".to_owned(),
            scope: Scope::Program {
                program: program.to_owned(),
            },
        }
    }

    #[test]
    fn a_session_lends_exactly_what_it_holds() {
        // A child gets what its parent already has and nothing more, all of it consented to once.
        let mut app = app();
        app.was_granted(run("git"));
        app.was_granted(run("cargo"));
        assert_eq!(app.lending(), vec![run("git"), run("cargo")]);
    }

    #[test]
    fn the_same_grant_twice_is_lent_once() {
        // Answering "any git command" to two prompts is one permission, not two.
        let mut app = app();
        app.was_granted(run("git"));
        app.was_granted(run("git"));
        assert_eq!(app.lending().len(), 1);
    }

    #[test]
    fn a_session_that_holds_nothing_lends_nothing() {
        // An empty list rather than an absent one: a child of a grantless session may do nothing.
        assert!(app().lending().is_empty());
    }

    #[test]
    fn what_a_person_answered_is_what_is_lent() {
        // Read through the same function the ledger uses, so the UI cannot drift from enforcement.
        let action = Action::Run {
            command: "git status".to_owned(),
            program: "git".to_owned(),
        };
        let scope = Scope::Program {
            program: "git".to_owned(),
        };
        let grant = magi_tools::permit::standing(&action, &scope).expect("a standing rule");
        let mut app = app();
        app.was_granted(grant);
        assert_eq!(app.lending(), vec![run("git")]);
    }

    #[test]
    fn answering_just_this_once_leaves_nothing_to_lend() {
        // `Once` is spent on the call that asked; lending it would outlive the parent's own.
        let action = Action::Run {
            command: "rm -rf /".to_owned(),
            program: "rm".to_owned(),
        };
        assert!(magi_tools::permit::standing(&action, &Scope::Once).is_none());
    }
}
