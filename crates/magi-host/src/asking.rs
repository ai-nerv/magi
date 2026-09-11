//! Putting a permission question to whoever is attached, from inside a turn. A tool runs on a
//! blocking thread deep inside a turn and the only person who can answer is behind an async socket,
//! so the question goes out as an event and the answer comes back on a plain [`std::sync::mpsc`]
//! receiver. Nobody attached means no: a question nobody can see is not a question.

use magi_proto::permit::{Action, Decision};
use magi_proto::{Cursor, HarnessEvent, ToolCallId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a question waits before it answers itself with a refusal.
const PATIENCE: Duration = Duration::from_secs(300);

/// The questions currently outstanding. Two maps, because a permission comes back as a
/// [`Decision`] and a general question as the id of a chosen option.
#[derive(Default)]
pub struct Pending {
    waiting: Mutex<HashMap<ToolCallId, std::sync::mpsc::Sender<Decision>>>,
    choosing: Mutex<HashMap<ToolCallId, std::sync::mpsc::Sender<String>>>,
}

impl Pending {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver an answer to whoever is waiting for it. An id nobody is waiting on is dropped: the
    /// turn it belonged to is over.
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

    /// Deliver a chosen option to whoever is waiting for it; an id nobody waits on is dropped.
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

/// The tool casper draws a permission with. Named here rather than in the config, because magi is
/// what opens it.
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
    /// Separate from [`Self::new`] because a magi with no casper has no surface to draw on.
    #[must_use]
    pub fn drawn_by(mut self, holds: Arc<dyn magi_tools::holding::Holds>) -> Self {
        self.holds = Some(holds);
        self
    }
}

impl magi_tools::approve::Approver for Asker {
    fn ask(&self, tool: &str, action: &Action) -> Decision {
        if !(self.attached)() {
            // Nobody is looking. Saying yes here would make the gate a formality on unwatched runs.
            return Decision::Deny;
        }
        // The prompt is a surface: magi decides that a permission is needed, casper draws it, and
        // what comes back is the id of a row that magi maps onto its own scopes here. Falling back
        // to the built-in picker when there is none, or every gated tool becomes a refusal.
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
    /// Put the permission on a surface, and read back what was chosen. `None` when there is no
    /// surface to put it on, and the caller falls back to the picker magi draws itself.
    fn through_a_surface(&self, tool: &str, action: &Action) -> Option<Decision> {
        let holds = self.holds.as_ref()?;
        let offers = magi_tools::permit::Ledger::offers(action);
        // Asked for rather than assumed, because only the thing drawing it knows how tall it is.
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
        // An id, mapped here. Anything that is not an offer's index is a refusal, which covers "no",
        // a surface that ended without answering, and a casper offering what this build cannot name.
        let decision = chosen
            .parse::<usize>()
            .ok()
            .and_then(|nth| offers.get(nth))
            .map_or(Decision::Deny, |scope| Decision::Allow {
                scope: scope.clone(),
                lifetime: magi_proto::permit::Lifetime::Session,
            });
        // Told to whoever is attached, because the UI is where a grant is remembered.
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
            // Nobody is looking, so nobody can answer; choosing on their behalf is what this prevents.
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

        // The same patience a permission gets. An unanswered question is not a refusal — the tool decides.
        let answer = receiver.recv_timeout(PATIENCE).ok();
        self.pending.drop_choice(&id);
        answer
    }
}

/// The three ways a turn reaches whoever is attached: a permission, a question, and rows a tool
/// draws in itself. They have never travelled apart.
#[derive(Clone)]
pub struct Person {
    pub approver: Arc<dyn magi_tools::approve::Approver>,
    pub asks: Arc<dyn magi_tools::question::Asks>,
    pub holds: Arc<dyn magi_tools::holding::Holds>,
    /// The surfaces currently on screen, so a keypress reaches the one holding the rows.
    pub surfaces: Arc<crate::holder::Holding>,
}

impl Person {
    /// Every face of one asker, plus the holder, which is not an asker: it spawns and pumps frames.
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
        // The trust boundary: no field it could set to "allowed", because scopes never leave this side.
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
        // The UI learns what this session holds from the answers it sends; this never passes through it.
        let action = running("cargo test");
        // A row that actually stands: "just this once" is not remembered, so it would test nothing.
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
        // A magi with no casper installed, falling through to the picker rather than refusing.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let asker = Asker::new(
            Arc::new(Pending::new()),
            Box::new(move |event| kept.lock().expect("held").push(event)),
            Box::new(|| Cursor::ZERO),
            Box::new(|| true),
        );
        // Nobody answers, so it times out into a refusal; the question is what is under test.
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
        // Every row the surface is given maps back to something.
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

/// Every gated tool, not only the ones that run commands. Tested through the real gate rather than
/// by calling the asker directly, because a second path is exactly what would go unnoticed.
#[cfg(test)]
mod every_verb {
    use super::permitting::{Fixed, asking};
    use magi_model::scratch::Scratch;
    use magi_proto::permit::Decision;

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
        // The subject is the permission surface, exercised through `ops.allow` — the seam every
        // tool goes through, magi's own and the tools program's alike, now that magi runs none.
        use magi_tools::ops::Ops as _;
        let dir = Scratch::new("magi-asking", "one");
        std::fs::write(dir.join("note.txt"), "hello").expect("a file");
        // "no", so nothing is granted and the test leaves no standing permission behind.
        let (ops, holder) = gated(&dir, Some("no".to_owned()));
        let out = ops.allow(
            "read",
            &magi_proto::permit::Action::Read {
                path: dir.join("note.txt").display().to_string(),
            },
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
        assert!(out.is_err(), "denied, so the read must not be allowed");
    }

    #[test]
    fn writing_a_file_puts_the_question_on_a_surface_too() {
        use magi_tools::ops::Ops as _;
        let dir = Scratch::new("magi-asking-w", "one");
        let (ops, holder) = gated(&dir, Some("no".to_owned()));
        let out = ops.allow(
            "write",
            &magi_proto::permit::Action::Write {
                path: dir.join("new.txt").display().to_string(),
            },
        );
        let args = shown(&holder).expect("the prompt was never drawn");
        assert_eq!(args["verb"], "write");
        assert!(out.is_err(), "a refused write must not be allowed");
    }

    #[test]
    fn a_yes_on_the_surface_lets_the_read_through() {
        // The other half: "the surface said allow" has to come back allowed.
        use magi_tools::ops::Ops as _;
        let dir = Scratch::new("magi-asking-y", "one");
        std::fs::write(dir.join("note.txt"), "hello").expect("a file");
        // Row zero, which is `Once` for every action: allowed, and nothing left standing.
        let (ops, _) = gated(&dir, Some("0".to_owned()));
        let out = ops.allow(
            "read",
            &magi_proto::permit::Action::Read {
                path: dir.join("note.txt").display().to_string(),
            },
        );
        assert!(out.is_ok(), "{out:?}");
    }

    #[test]
    fn the_ledger_still_answers_the_second_time() {
        // The surface is asked once: moving the prompt out of magi must not move the remembering.
        use magi_tools::ops::Ops as _;
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
            let out = ops.allow(
                "read",
                &magi_proto::permit::Action::Read {
                    path: dir.join(file).display().to_string(),
                },
            );
            assert!(out.is_ok(), "{file}: {out:?}");
        }
        assert_eq!(
            holder.shown.lock().expect("held").len(),
            1,
            "asked twice about one directory"
        );
    }

    #[test]
    fn nobody_attached_is_still_a_refusal_whoever_would_have_drawn_it() {
        // The surface must not have opened a way round it: a question nobody sees is not a question.
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
