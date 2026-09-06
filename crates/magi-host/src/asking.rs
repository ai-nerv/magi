//! Putting a permission question to whoever is attached, from inside a turn.
//!
//! The awkward shape this solves: a tool runs on a blocking thread deep inside a turn, and the
//! only person who can answer it is on the other end of a socket being served by an async loop.
//! Neither end can call the other directly.
//!
//! So the question goes out as an event and the answer comes back on a channel. The tool blocks
//! on a plain [`std::sync::mpsc`] receiver — it is not async and must not become async for this
//! — and the command loop, which *is* async, drops the answer into it with a non-blocking send.
//!
//! **Nobody attached means no.** A question nobody can see is not a question, and answering it
//! on their behalf is the whole failure this mechanism exists to prevent.

use magi_proto::permit::{Action, Decision};
use magi_proto::{Cursor, HarnessEvent, ToolCallId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a question waits before it answers itself with a refusal.
///
/// Long, because somebody may be reading it. Bounded, because a turn that waits forever on a UI
/// that has gone is a daemon nothing can recover.
const PATIENCE: Duration = Duration::from_secs(300);

/// The questions currently outstanding.
///
/// Two maps, because the two kinds of question carry different answers: a permission comes back
/// as a [`Decision`], and a general one as the id of a chosen option. One map holding an enum of
/// both would make every reader unpack a thing it already knows the shape of.
#[derive(Default)]
pub struct Pending {
    waiting: Mutex<HashMap<ToolCallId, std::sync::mpsc::Sender<Decision>>>,
    choosing: Mutex<HashMap<ToolCallId, std::sync::mpsc::Sender<String>>>,
}

impl Pending {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver an answer to whoever is waiting for it.
    ///
    /// An id nobody is waiting on is dropped: the turn it belonged to is over, and acting on it
    /// would allow something nobody is watching.
    pub fn answer(&self, id: &ToolCallId, decision: Decision) {
        let Ok(mut waiting) = self.waiting.lock() else {
            return;
        };
        if let Some(sender) = waiting.remove(id) {
            let _ = sender.send(decision);
        }
    }

    /// Register a question and hand back the end to wait on.
    fn register(&self, id: ToolCallId) -> Option<std::sync::mpsc::Receiver<Decision>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.waiting.lock().ok()?.insert(id, sender);
        Some(receiver)
    }

    /// Deliver a chosen option to whoever is waiting for it.
    ///
    /// An id nobody is waiting on is dropped, for the same reason a permission's is: the turn it
    /// belonged to is over, and acting on it would resume something nobody is watching.
    pub fn chose(&self, id: &ToolCallId, choice: String) {
        let Ok(mut choosing) = self.choosing.lock() else {
            return;
        };
        if let Some(sender) = choosing.remove(id) {
            let _ = sender.send(choice);
        }
    }

    /// Register a general question and hand back the end to wait on.
    fn awaiting(&self, id: ToolCallId) -> Option<std::sync::mpsc::Receiver<String>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.choosing.lock().ok()?.insert(id, sender);
        Some(receiver)
    }

    /// Forget a chosen-option question.
    fn drop_choice(&self, id: &ToolCallId) {
        if let Ok(mut choosing) = self.choosing.lock() {
            choosing.remove(id);
        }
    }

    /// Forget a question, so a timed-out one does not sit in the map for the session.
    fn forget(&self, id: &ToolCallId) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.remove(id);
        }
    }
}

/// The tool casper draws a permission with.
///
/// Named here rather than in the config, because magi is what opens it: a name only one side knew
/// would be a prompt that silently stopped appearing the day somebody renamed it.
const PROMPT: &str = "permission";

/// Asks by publishing an event, and waits on the channel.
pub struct Asker {
    pending: Arc<Pending>,
    holds: Option<Arc<dyn magi_tools::holding::Holds>>,
    publish: Box<dyn Fn(HarnessEvent) + Send + Sync>,
    cursor: Box<dyn Fn() -> Cursor + Send + Sync>,
    attached: Box<dyn Fn() -> bool + Send + Sync>,
    next: std::sync::atomic::AtomicU64,
}

impl Asker {
    /// An asker that publishes with `publish` and numbers its questions from zero.
    #[must_use]
    pub fn new(
        pending: Arc<Pending>,
        publish: Box<dyn Fn(HarnessEvent) + Send + Sync>,
        cursor: Box<dyn Fn() -> Cursor + Send + Sync>,
        attached: Box<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            pending,
            holds: None,
            publish,
            cursor,
            attached,
            next: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The same, drawing its permission prompt on a surface rather than in magi's own picker.
    ///
    /// Separate from [`Self::new`] because a magi with no casper has no surface to draw on, and
    /// an asker that required one could not ask at all.
    #[must_use]
    pub fn drawn_by(mut self, holds: Arc<dyn magi_tools::holding::Holds>) -> Self {
        self.holds = Some(holds);
        self
    }
}

impl magi_tools::approve::Approver for Asker {
    fn ask(&self, tool: &str, action: &Action) -> Decision {
        if !(self.attached)() {
            // Nobody is looking. Saying yes here would make the gate a formality on exactly the
            // sessions nobody is watching, which are the ones it matters on.
            return Decision::Deny;
        }
        // **Drawn by whoever can draw it.** The prompt is a surface now: magi decides that a
        // permission is needed and what it is about, and casper draws the question and collects
        // the keystroke. What comes back is the id of a row — magi maps that onto its own scopes
        // here, because a sibling that answered "allowed" would make this ledger a suggestion.
        //
        // Falling back to the built-in picker when there is no surface to be had. A magi with no
        // casper installed must still be able to ask, or every gated tool becomes a refusal.
        if let Some(decision) = self.through_a_surface(tool, action) {
            return decision;
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("p{n}"));
        let Some(receiver) = self.pending.register(id.clone()) else {
            return Decision::Deny;
        };

        (self.publish)(HarnessEvent::PermissionAsked {
            cursor: (self.cursor)(),
            id: id.clone(),
            tool: tool.to_owned(),
            action: action.clone(),
            offers: magi_tools::permit::Ledger::offers(action),
        });

        let answer = receiver.recv_timeout(PATIENCE).unwrap_or(Decision::Deny);
        self.pending.forget(&id);
        answer
    }
}

impl Asker {
    /// Put the permission on a surface, and read back what was chosen.
    ///
    /// `None` when there is no surface to put it on — no casper, or nobody attached — and the
    /// caller falls back to the picker magi draws itself.
    fn through_a_surface(&self, tool: &str, action: &Action) -> Option<Decision> {
        let holds = self.holds.as_ref()?;
        let offers = magi_tools::permit::Ledger::offers(action);
        // The rows a prompt this size needs: a heading, the subject, a blank, a row per offer and
        // the Deny beneath them, and the line saying which keys do what. Asked for rather than
        // assumed, because only the thing drawing it knows how tall it is.
        let rows = u16::try_from(offers.len() + 6).unwrap_or(u16::MAX);
        let mut rows_json: Vec<serde_json::Value> = offers
            .iter()
            .enumerate()
            .map(|(nth, scope)| {
                serde_json::json!({"id": nth.to_string(), "label": scope.label(action)})
            })
            .collect();
        rows_json.push(serde_json::json!({
            "id": "no",
            "label": "Deny",
            "about": "the model is told, and carries on",
        }));
        let asked = magi_proto::tooling::Surface {
            rows,
            about: format!("{tool} wants to {} {}", action.verb(), action.subject()),
            // No tick: a prompt redraws when a key arrives and at no other time.
            tick: None,
        };
        let chosen = holds.hold(
            PROMPT,
            &asked,
            &serde_json::json!({
                "tool": tool,
                "verb": action.verb(),
                "subject": action.subject(),
                "offers": rows_json,
            }),
        )?;
        // **An id, mapped here.** Anything that is not an offer's index is a refusal, which covers
        // "no", a surface that ended without answering, and a casper newer than this build
        // offering something it has no name for. Denying is the safe reading of all three.
        let decision = chosen
            .parse::<usize>()
            .ok()
            .and_then(|nth| offers.get(nth))
            .map_or(Decision::Deny, |scope| Decision::Allow {
                scope: scope.clone(),
                lifetime: magi_proto::permit::Lifetime::Session,
            });
        // Told to whoever is attached, because the UI is where a grant is remembered and this one
        // was decided on the tool thread. Without it a session that lends its permissions to a
        // child would lend everything answered at a picker and nothing answered at a surface.
        if let Decision::Allow { scope, .. } = &decision
            && let Some(grant) = magi_tools::permit::standing(action, scope)
        {
            (self.publish)(HarnessEvent::Granted {
                cursor: (self.cursor)(),
                grant,
            });
        }
        Some(decision)
    }
}

impl magi_tools::question::Asks for Asker {
    fn ask(&self, tool: &str, ask: &magi_proto::tooling::Ask) -> Option<String> {
        if !(self.attached)() {
            // Nobody is looking, so nobody can answer. Choosing on their behalf is the failure
            // this mechanism exists to prevent, and it matters most on exactly the sessions
            // where nobody is watching.
            return None;
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("q{n}"));
        let receiver = self.pending.awaiting(id.clone())?;

        (self.publish)(HarnessEvent::Asked {
            cursor: (self.cursor)(),
            id: id.clone(),
            tool: tool.to_owned(),
            question: ask.question.clone(),
            options: ask.options.clone(),
            detail: ask.detail.clone(),
        });

        // The same patience a permission gets. A turn that waited forever on a UI that has gone
        // is a daemon nothing can recover, and an unanswered question is not a refusal — the
        // tool decides what to make of it.
        let answer = receiver.recv_timeout(PATIENCE).ok();
        self.pending.drop_choice(&id);
        answer
    }
}

/// The three ways a turn reaches whoever is attached.
///
/// A permission, a question, and rows a tool draws in itself. They have never travelled apart —
/// each is the same UI seen from a different distance — and carrying them as three parameters
/// said they were three things.
#[derive(Clone)]
pub struct Person {
    /// Asked before a tool that needs permission runs.
    pub approver: Arc<dyn magi_tools::approve::Approver>,
    /// Asked when a tool has a question of its own.
    pub asks: Arc<dyn magi_tools::question::Asks>,
    /// Given the rows when a tool wants to draw its own.
    pub holds: Arc<dyn magi_tools::holding::Holds>,
    /// The surfaces currently on screen, so a keypress reaches the one holding the rows.
    ///
    /// Here rather than a parameter of its own because it travels with the rest: the command loop
    /// that delivers an answer to a question is the loop that delivers a key to a surface.
    pub surfaces: Arc<crate::holder::Holding>,
}

impl Person {
    /// Every face of one asker, and the holder that gives out rows.
    ///
    /// The holder is separate because it is not an asker: it spawns a process and pumps frames,
    /// which has nothing to do with putting a question on a channel.
    #[must_use]
    pub fn of(
        asker: Arc<Asker>,
        holds: Arc<dyn magi_tools::holding::Holds>,
        surfaces: Arc<crate::holder::Holding>,
    ) -> Self {
        Self {
            approver: Arc::clone(&asker) as Arc<_>,
            asks: asker as Arc<_>,
            holds,
            surfaces,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_tools::approve::Approver;

    fn asker(attached: bool) -> (Arc<Pending>, Asker, Arc<Mutex<Vec<HarnessEvent>>>) {
        let pending = Arc::new(Pending::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let asker = Asker::new(
            Arc::clone(&pending),
            Box::new(move |event| {
                if let Ok(mut seen) = kept.lock() {
                    seen.push(event);
                }
            }),
            Box::new(|| Cursor(1)),
            Box::new(move || attached),
        );
        (pending, asker, seen)
    }

    fn read() -> Action {
        Action::Read {
            path: "/etc/shadow".to_owned(),
        }
    }

    #[test]
    fn nobody_attached_is_a_refusal_and_asks_nothing() {
        // A question nobody can see is not a question.
        let (_, asker, seen) = asker(false);
        assert_eq!(asker.ask("read", &read()), Decision::Deny);
        assert!(
            seen.lock().expect("lock").is_empty(),
            "and it is not published"
        );
    }

    #[test]
    fn a_question_is_published_with_the_widths_it_can_be_answered_at() {
        let (pending, asker, seen) = asker(true);
        let answering = std::thread::spawn(move || {
            // Wait for it to register, then answer.
            for _ in 0..200 {
                let id = ToolCallId::new("p0");
                pending.answer(
                    &id,
                    Decision::Allow {
                        scope: magi_proto::permit::Scope::Once,
                        lifetime: magi_proto::permit::Lifetime::Session,
                    },
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let decision = asker.ask("read", &read());
        assert!(matches!(decision, Decision::Allow { .. }));

        let seen = seen.lock().expect("lock");
        let HarnessEvent::PermissionAsked { action, offers, .. } = seen.first().expect("published")
        else {
            panic!("wrong event");
        };
        assert_eq!(action, &read());
        assert!(offers.len() >= 2, "narrow and broad, not just yes");
        drop(seen);
        let _ = answering.join();
    }

    #[test]
    fn an_answer_nobody_is_waiting_for_is_dropped() {
        // The turn it belonged to is over; acting on it would allow something unwatched.
        let pending = Pending::new();
        pending.answer(&ToolCallId::new("gone"), Decision::Deny);
    }
}

/// The permission prompt, drawn by somebody else and decided here.
#[cfg(test)]
mod permitting {
    use super::*;
    use magi_proto::permit::{Lifetime, Scope};
    use magi_tools::approve::Approver;

    /// A holder that answers every surface with `chosen`, and records what it was shown.
    pub(super) struct Fixed {
        chosen: Option<String>,
        pub(super) shown: Mutex<Vec<serde_json::Value>>,
    }

    impl magi_tools::holding::Holds for Fixed {
        fn hold(
            &self,
            _tool: &str,
            _surface: &magi_proto::tooling::Surface,
            args: &serde_json::Value,
        ) -> Option<String> {
            self.shown.lock().expect("held").push(args.clone());
            self.chosen.clone()
        }
    }

    /// An asker whose prompt is drawn by `chosen`, and the events it publishes.
    pub(super) fn asking(
        chosen: Option<String>,
    ) -> (Asker, Arc<Fixed>, Arc<Mutex<Vec<HarnessEvent>>>) {
        let holder = Arc::new(Fixed {
            chosen,
            shown: Mutex::new(Vec::new()),
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let asker = Asker::new(
            Arc::new(Pending::new()),
            Box::new(move |event| kept.lock().expect("held").push(event)),
            Box::new(|| Cursor::ZERO),
            Box::new(|| true),
        )
        .drawn_by(Arc::clone(&holder) as Arc<_>);
        (asker, holder, seen)
    }

    fn running(command: &str) -> Action {
        Action::Run {
            command: command.to_owned(),
            program: command.split(' ').next().unwrap_or(command).to_owned(),
        }
    }

    #[test]
    fn the_surface_is_shown_what_is_being_decided_and_told_nothing_it_could_decide_with() {
        // The trust boundary. It gets the tool, the verb, the subject and the rows to draw — and
        // no field it could set to "allowed", because the scopes never leave this side.
        let (asker, holder, _) = asking(Some("no".to_owned()));
        asker.ask("shell", &running("rm -rf build"));
        let shown = holder.shown.lock().expect("held");
        let args = shown.first().expect("it was shown something");
        assert_eq!(args["tool"], "shell");
        assert_eq!(args["subject"], "rm -rf build");
        let wire = args.to_string();
        for granting in ["\"scope\"", "\"lifetime\"", "\"allow\"", "\"decision\""] {
            assert!(!wire.contains(granting), "{granting} crossed: {wire}");
        }
    }

    #[test]
    fn the_id_a_surface_returns_is_mapped_onto_a_scope_here() {
        // It answers with the index of a row it drew. What that *means* is worked out on this
        // side, from the offers this side produced.
        let action = running("cargo test");
        let offers = magi_tools::permit::Ledger::offers(&action);
        let (asker, _, _) = asking(Some("0".to_owned()));
        assert_eq!(
            asker.ask("shell", &action),
            Decision::Allow {
                scope: offers[0].clone(),
                lifetime: Lifetime::Session,
            }
        );
    }

    #[test]
    fn an_answer_that_names_no_offer_is_a_refusal() {
        // Covers "no", a surface that ended without answering, and a casper newer than this build
        // offering something it has no name for. Denying is the safe reading of all three.
        for said in ["no", "", "17", "allow-everything"] {
            let (asker, _, _) = asking(Some(said.to_owned()));
            assert_eq!(
                asker.ask("shell", &running("rm -rf /")),
                Decision::Deny,
                "{said:?}"
            );
        }
    }

    #[test]
    fn a_grant_made_on_a_surface_is_still_told_to_the_screen() {
        // The UI remembers what this session holds so a child can be lent it, and it learns that
        // from the answers it sends. This one never passes through it.
        let action = running("cargo test");
        // A row that actually stands. "just this once" is not remembered and should not be —
        // there is nothing standing about it — so choosing it here would test nothing.
        let nth = magi_tools::permit::Ledger::offers(&action)
            .iter()
            .position(|scope| magi_tools::permit::standing(&action, scope).is_some())
            .expect("something on offer outlasts the call");
        let (asker, _, seen) = asking(Some(nth.to_string()));
        asker.ask("shell", &action);
        assert!(
            seen.lock()
                .expect("held")
                .iter()
                .any(|event| matches!(event, HarnessEvent::Granted { .. })),
            "nothing said a grant was made"
        );
    }

    #[test]
    fn without_a_surface_the_question_still_gets_asked() {
        // A magi with no casper installed. Falling through to the picker rather than refusing,
        // because an asker that could not ask would make every gated tool a refusal.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let asker = Asker::new(
            Arc::new(Pending::new()),
            Box::new(move |event| kept.lock().expect("held").push(event)),
            Box::new(|| Cursor::ZERO),
            Box::new(|| true),
        );
        // Nobody answers, so it times out into a refusal — but the *question* is what is under
        // test, and it was published.
        std::thread::spawn(move || asker.ask("shell", &running("ls")));
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            seen.lock()
                .expect("held")
                .iter()
                .any(|event| matches!(event, HarnessEvent::PermissionAsked { .. })),
            "no question reached the screen"
        );
    }

    #[test]
    fn a_scope_offered_is_a_scope_that_can_be_chosen() {
        // Every row the surface is given maps back to something, or a person could pick a row
        // that quietly did nothing.
        let action = running("git status");
        let offers = magi_tools::permit::Ledger::offers(&action);
        for (nth, scope) in offers.iter().enumerate() {
            let (asker, _, _) = asking(Some(nth.to_string()));
            assert_eq!(
                asker.ask("shell", &action),
                Decision::Allow {
                    scope: scope.clone(),
                    lifetime: Lifetime::Session,
                },
                "row {nth} did not map back"
            );
        }
        assert!(
            offers.iter().any(|s| matches!(s, Scope::Once)),
            "{offers:?}"
        );
    }
}

/// Every gated tool, not only the ones that run commands.
///
/// The claim this file makes is that a permission is a surface now. Tested through the real
/// gate — the one `read`, `write` and every casper tool go through — rather than by calling the
/// asker directly, because "the same `Approver` is used for all of them" is exactly the kind of
/// thing that stays true right up until somebody adds a second path.
#[cfg(test)]
mod every_verb {
    use super::permitting::{Fixed, asking};
    use magi_model::scratch::Scratch;
    use magi_proto::permit::Decision;
    use magi_tools::registry::Tool;

    /// A gated session rooted at `dir`, whose prompt is answered with `chosen`.
    fn gated(
        dir: &std::path::Path,
        chosen: Option<String>,
    ) -> (magi_tools::ops::Real, std::sync::Arc<Fixed>) {
        let (asker, holder, _) = asking(chosen);
        let ops = magi_tools::ops::Real::gated(
            dir.to_path_buf(),
            magi_tools::permit::Ledger::new(),
            std::sync::Arc::new(asker),
        );
        (ops, holder)
    }

    /// What the prompt was shown, if it was shown anything.
    fn shown(holder: &Fixed) -> Option<serde_json::Value> {
        holder.shown.lock().expect("held").first().cloned()
    }

    #[test]
    fn reading_a_file_puts_the_question_on_a_surface() {
        let dir = Scratch::new("magi-asking", "one");
        std::fs::write(dir.join("note.txt"), "hello").expect("a file");
        // "no", so nothing is granted and the test leaves no standing permission behind.
        let (ops, holder) = gated(&dir, Some("no".to_owned()));
        let out = magi_tools::builtin::Read.run(
            &serde_json::json!({"path": "note.txt"}),
            &ops,
            &magi_tools::cancel::Uncancelled,
        );
        let args = shown(&holder).expect("the prompt was never drawn");
        assert_eq!(args["tool"], "read");
        assert_eq!(args["verb"], "read");
        assert!(
            args["subject"]
                .as_str()
                .unwrap_or_default()
                .ends_with("note.txt"),
            "{args}"
        );
        assert!(out.is_error, "denied, so the read must not have happened");
    }

    #[test]
    fn writing_a_file_puts_the_question_on_a_surface_too() {
        let dir = Scratch::new("magi-asking-w", "one");
        let (ops, holder) = gated(&dir, Some("no".to_owned()));
        let out = magi_tools::builtin::Write.run(
            &serde_json::json!({"path": "new.txt", "contents": "x"}),
            &ops,
            &magi_tools::cancel::Uncancelled,
        );
        let args = shown(&holder).expect("the prompt was never drawn");
        assert_eq!(args["verb"], "write");
        assert!(out.is_error);
        assert!(
            !dir.join("new.txt").exists(),
            "a refused write happened anyway"
        );
    }

    #[test]
    fn a_yes_on_the_surface_lets_the_read_through() {
        // The other half. A prompt that could only refuse would pass the test above and be
        // useless, and "the surface said allow" has to reach the file.
        let dir = Scratch::new("magi-asking-y", "one");
        std::fs::write(dir.join("note.txt"), "hello").expect("a file");
        // Row zero, which is `Once` for every action: allowed, and nothing left standing.
        let (ops, _) = gated(&dir, Some("0".to_owned()));
        let out = magi_tools::builtin::Read.run(
            &serde_json::json!({"path": "note.txt"}),
            &ops,
            &magi_tools::cancel::Uncancelled,
        );
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("hello"), "{}", out.content);
    }

    #[test]
    fn the_ledger_still_answers_the_second_time() {
        // The surface is asked once. A prompt per file in a directory somebody already allowed
        // is the difference between a permission and a nuisance, and moving the prompt out of
        // magi must not have moved the remembering with it.
        let dir = Scratch::new("magi-asking-l", "one");
        std::fs::write(dir.join("a.txt"), "one").expect("a file");
        std::fs::write(dir.join("b.txt"), "two").expect("a file");
        let action = magi_proto::permit::Action::Read {
            path: dir.join("a.txt").display().to_string(),
        };
        // The directory the file is in, which is what covers the sibling beside it.
        let nth = magi_tools::permit::Ledger::offers(&action)
            .iter()
            .position(|scope| {
                matches!(scope, magi_proto::permit::Scope::Directory { path } if *path == dir.display().to_string())
            })
            .expect("the containing directory is on offer");
        let (ops, holder) = gated(&dir, Some(nth.to_string()));
        for file in ["a.txt", "b.txt"] {
            let out = magi_tools::builtin::Read.run(
                &serde_json::json!({"path": file}),
                &ops,
                &magi_tools::cancel::Uncancelled,
            );
            assert!(!out.is_error, "{file}: {}", out.content);
        }
        assert_eq!(
            holder.shown.lock().expect("held").len(),
            1,
            "asked twice about one directory"
        );
    }

    #[test]
    fn nobody_attached_is_still_a_refusal_whoever_would_have_drawn_it() {
        // The rule the whole mechanism exists for, and the surface must not have opened a way
        // round it: a question nobody can see is not a question.
        let asker = super::Asker::new(
            std::sync::Arc::new(super::Pending::new()),
            Box::new(|_| {}),
            Box::new(|| magi_proto::Cursor::ZERO),
            Box::new(|| false),
        );
        use magi_tools::approve::Approver;
        assert_eq!(
            asker.ask(
                "read",
                &magi_proto::permit::Action::Read {
                    path: "/etc/shadow".to_owned()
                }
            ),
            Decision::Deny
        );
    }
}
