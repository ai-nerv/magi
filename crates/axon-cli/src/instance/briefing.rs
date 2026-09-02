//! Telling the model an instance was named.
//!
//! `$iota-mu` in a prompt does not send anything. It appends a note saying that instance exists,
//! how it stands to this session, what has already passed between them, and that there is a tool
//! for reaching it — and then the model decides what "tell it to stop" meant and calls the tool,
//! or does not.
//!
//! That order matters and it is the whole design. A harness that delivered the message itself
//! would be deciding what the sentence meant: whether "ask $iota-mu about the parser" is a
//! question to relay, a plan to make first, or a thing to do after reading a file. Naming an
//! instance is a *fact given to the model*, exactly like naming a file is.
//!
//! What is appended is bounded and it is not a system prompt. It is the smallest thing that
//! turns a name into something callable: who they are, how they stand to you, whether they can
//! be reached at all, what has already been said, and the name of the tool.

use super::policy::{self, Relation};
use super::{Address, Reach};
use crate::app::App;

/// How many earlier messages with an instance are worth repeating.
///
/// Enough to make a reply make sense and not so many that naming a long-running sibling costs
/// the turn its context. The whole exchange is in the journal; this is the part that reads as
/// conversation.
const RECALLED: usize = 6;

/// What the model needs in order to act on the instances a prompt names.
///
/// Empty when it names none, which is almost every prompt — and empty is what makes this an
/// aside rather than an edit: nothing is added to the prompt, so there is nothing to strip back
/// out and no way for the two to disagree about where one ends.
#[must_use]
pub fn about(text: &str, app: &App) -> String {
    let named = axon_tui::trigger::named(text, axon_tui::trigger::Trigger::Instance);
    if named.is_empty() {
        return String::new();
    }
    let mut said = Vec::new();
    for name in named {
        let Some(address) = Address::read(&name) else {
            said.push(format!(
                "`${name}` is not a name an instance can have, so nothing answers to it. \
                 Names are `id`, `role/id` or `project/role/id`."
            ));
            continue;
        };
        said.push(brief(&address, app));
    }
    said.push(String::new());
    said.push(format!(
        "Use the `{}` tool to reach any of them. Call `{}` with `verb: \"help\"` \
         to see everything it can do.",
        super::TOOL,
        super::TOOL
    ));
    said.join("\n")
}

/// What is known about one instance, as a paragraph the model can act on.
fn brief(address: &Address, app: &App) -> String {
    let whole = address.against(&app.identity);
    let me = app.whom();
    // Through the same [`Standing`](super::tool::Standing) the tool will use, so the briefing
    // and the refusal can never disagree about where somebody sits. A model told it may stop a
    // session and then refused when it tries has been lied to by the harness.
    let relation = app.standing().stands(&whole);
    let mut said = format!(
        "`{}` is another axon, addressed as `{}`. It is {}.",
        whole.full(),
        address.written(),
        relation.named()
    );
    // Said before anything else about it, because a model that reads the history and then meets
    // a refusal has spent the turn planning something it was never going to be allowed to do.
    if !policy::may(&me, relation, Reach::Ask) {
        said.push_str(" This session cannot reach it: ");
        said.push_str(&policy::refusal(&me, relation, Reach::Ask));
        said.push('.');
        return said;
    }
    if relation == Relation::Child {
        said.push_str(" This session started it, so it can be stopped as well as asked.");
    } else {
        said.push_str(" It can be asked and told things, but not stopped.");
    }
    let passed: Vec<String> = app
        .inbox
        .iter()
        .filter(|message| message.from == whole.full())
        .rev()
        .take(RECALLED)
        .map(|message| format!("  - it said: {}", message.text))
        .collect();
    if !passed.is_empty() {
        said.push_str("\n\n  What has already passed between you:\n");
        // Oldest first, because that is the order it happened in and the order a reply reads.
        said.push_str(&passed.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    said
}

/// Naming an instance adds a fact, and nothing else moves.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::wire::Message;

    fn app() -> App {
        let mut app = App::new();
        app.identity = crate::identity::Identity {
            project: "axon".to_owned(),
            role: "main".to_owned(),
            id: "alpha-rho".to_owned(),
        };
        app
    }

    #[test]
    fn a_prompt_that_names_nobody_produces_nothing_at_all() {
        // Almost every prompt. Anything here is context somebody pays for, and an empty aside
        // is what keeps a prompt that named nobody indistinguishable from one typed before any
        // of this existed.
        let app = app();
        for text in ["fix the parser", "look at @src/main.rs", "it cost me 20$"] {
            assert!(about(text, &app).is_empty(), "{text:?} produced an aside");
        }
    }

    #[test]
    fn naming_one_says_what_is_known_about_it_and_nothing_of_the_prompt() {
        // The prompt is the model's to answer, and the person's to read. This goes beside it:
        // spliced onto the end, it put a page of facts into the transcript under their name.
        let said = about("tell $beta-nu to stop", &app());
        assert!(!said.contains("tell $beta-nu to stop"), "{said}");
        assert!(said.contains("axon/main/beta-nu"), "{said}");
    }

    #[test]
    fn it_names_the_tool_rather_than_doing_anything() {
        // The point of the whole file. The model decides what "tell it to stop" meant.
        let said = about("tell $beta-nu to stop", &app());
        assert!(said.contains(super::super::TOOL), "{said}");
    }

    #[test]
    fn it_says_how_the_named_one_stands_to_this_session() {
        // Otherwise the model tries, is refused, and spends a turn finding out something the
        // harness knew before it asked.
        let said = about("ask $beta-nu", &app());
        assert!(said.contains("another instance's main"), "{said}");
        assert!(said.contains("not stopped"), "{said}");
    }

    #[test]
    fn one_it_cannot_reach_says_so_instead_of_its_history() {
        // A model that reads the history and then meets a refusal has spent the turn planning
        // something it was never going to be allowed to do.
        let mut app = app();
        app.identity.project = "somewhere-with-no-runtime-dir".to_owned();
        app.parent = Some("beta-nu".to_owned());
        let said = about("ask $other/tau-chi", &app);
        assert!(said.contains("cannot reach"), "{said}");
    }

    #[test]
    fn what_has_already_been_said_comes_back_oldest_first() {
        // A reply reads in the order it happened. Reversed, the model answers the first message
        // as though it were the last.
        let mut app = app();
        for text in ["first", "second", "third"] {
            app.inbox.push(Message::new("axon/main/beta-nu", text));
        }
        let said = about("what did $beta-nu want", &app);
        let first = said.find("first").expect("the first is there");
        let third = said.find("third").expect("the third is there");
        assert!(first < third, "they came back backwards: {said}");
    }

    #[test]
    fn only_that_instance_s_messages_are_repeated() {
        let mut app = app();
        app.inbox
            .push(Message::new("axon/main/beta-nu", "from beta"));
        app.inbox
            .push(Message::new("axon/main/gamma-xi", "from gamma"));
        let said = about("what did $beta-nu want", &app);
        assert!(said.contains("from beta"), "{said}");
        assert!(!said.contains("from gamma"), "it leaked another's: {said}");
    }

    #[test]
    fn a_long_exchange_is_cut_rather_than_pasted_whole() {
        // Naming a long-running sibling must not cost the turn its context.
        let mut app = app();
        for at in 0..50 {
            app.inbox
                .push(Message::new("axon/main/beta-nu", &format!("message {at}")));
        }
        let said = about("what did $beta-nu want", &app);
        assert!(!said.contains("message 0"), "it pasted the whole exchange");
        assert!(said.contains("message 49"), "it dropped the newest");
    }

    #[test]
    fn a_name_nothing_can_have_is_said_to_be_one() {
        let said = about("ask $a/b/c/d about it", &app());
        assert!(said.contains("not a name"), "{said}");
    }
}
