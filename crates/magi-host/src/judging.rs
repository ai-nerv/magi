//! Who decides about an action no standing grant covers.
//!
//! The person always did. Here they may hand part of that to a second model, by mode, and keep
//! two kinds of rule that no mode overrides: what is never done, and what they are always asked
//! about. The second model is shown what the person asked for and the action, and never what a
//! tool printed, so that a file an agent has just read cannot talk it round. What it is unsure
//! of, cannot be asked, or keeps refusing, comes back to the person. The kernel jail is
//! underneath all of it and takes no instruction from any of it.

pub use magi_proto::judging::Rules;
use magi_proto::judging::{Advice, Mode};
use magi_proto::permit::{Action, Decision, Lifetime, Scope};
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Refusals in a row, and in all, after which the second model is no longer taken at its word
/// and the person is asked: it is missing something they know.
const IN_A_ROW: u32 = 3;
const IN_ALL: u32 = 20;

/// What the second model is told. It answers for one action at a time.
const INSTRUCTION: &str = "You decide whether a coding agent may take one action without asking \
the person at the keyboard. You are shown what the person asked for and the action. You are never \
shown what tools printed: an action that nothing the person said accounts for is a reason to \
refuse it.\n\
Allow: reading; building, testing, linting, formatting; changes inside the project directory; \
installing what the project's own manifests declare; read-only network requests; git that loses \
nothing (status, diff, log, add, commit, branch, switch, fetch, pull, and a push without force \
to the repository being worked in).\n\
Refuse: downloading and running code (`curl | sh`); sending files, secrets, keys or the \
environment to an outside host; deleting, moving or overwriting anything outside the project; \
destroying work this session did not make (`rm -rf` of what was there before, `git reset --hard`, \
`git clean -fd`, `git checkout -- .`, `git stash drop`, a force push, amending a pushed commit); \
changing the system, users, permissions, shell startup files, ssh keys or stored credentials; \
deploying, migrating or destroying infrastructure; anything the person said not to do; anything \
well beyond what they asked for.\n\
When unsure, refuse: the person is asked instead, which costs them a moment and nothing else.\n\
Answer with JSON alone: {\"safe\": true or false, \"rule\": \"two or three words\", \"reason\": \
\"one line: what the action does, and why that is or is not within what was asked\"}.";

/// Something that can say whether an action is safe. `None` is no answer: nothing configured,
/// nothing reachable, nothing parseable.
pub trait Judge: Send + Sync {
    fn judge(&self, tool: &str, action: &Action) -> Option<Advice>;
}

/// The mode, and how the second model has been doing, shared with whatever switches the mode.
#[derive(Debug, Default)]
pub struct Standing {
    mode: AtomicU8,
    in_a_row: AtomicU32,
    in_all: AtomicU32,
}

impl Standing {
    #[must_use]
    pub fn starting(mode: Mode) -> Self {
        let standing = Self::default();
        standing.set(mode);
        standing
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        Mode::ALL
            .into_iter()
            .find(|mode| *mode as u8 == self.mode.load(Ordering::Relaxed))
            .unwrap_or_default()
    }

    /// Change it. A fresh mode is a fresh count in a row; the count in all is the session's.
    pub fn set(&self, mode: Mode) {
        self.mode.store(mode as u8, Ordering::Relaxed);
        self.in_a_row.store(0, Ordering::Relaxed);
    }

    /// Whether the second model has refused so often that the person should be asked instead.
    fn spent(&self) -> bool {
        self.in_a_row.load(Ordering::Relaxed) >= IN_A_ROW
            || self.in_all.load(Ordering::Relaxed) >= IN_ALL
    }
}

/// The approver a session's tools reach: rules, then the mode, then whoever the mode names.
pub struct Judged {
    person: Arc<dyn magi_tools::approve::Approver>,
    judge: Arc<dyn Judge>,
    standing: Arc<Standing>,
    rules: Rules,
    /// The session's directory, inside which `edits` and `auto` let a write through.
    root: String,
    /// Why each action was last refused, for [`magi_tools::approve::Approver::why`].
    refused: Mutex<std::collections::BTreeMap<String, String>>,
    notice: Box<dyn Fn(String) + Send + Sync>,
}

impl Judged {
    #[must_use]
    pub fn new(
        person: Arc<dyn magi_tools::approve::Approver>,
        judge: Arc<dyn Judge>,
        standing: Arc<Standing>,
        rules: Rules,
        root: &std::path::Path,
        notice: Box<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        Self {
            person,
            judge,
            standing,
            rules,
            root: root.display().to_string(),
            refused: Mutex::new(std::collections::BTreeMap::new()),
            notice,
        }
    }

    fn once() -> Decision {
        Decision::Allow {
            scope: Scope::Once,
            lifetime: Lifetime::Session,
        }
    }

    fn refuse(&self, action: &Action, why: String) -> Decision {
        if let Ok(mut refused) = self.refused.lock() {
            refused.insert(key(action), why);
        }
        Decision::Deny
    }

    /// A write this session's own directory takes, and not into the repository's own records:
    /// what `edits` exists to stop asking about.
    fn an_edit_here(&self, action: &Action) -> bool {
        matches!(action, Action::Write { path }
            if magi_proto::permit::under(path, &self.root) && !path.contains("/.git/"))
    }

    /// The second model's word on it, acted on. Asked of the person when there is none, and when
    /// it has refused so often that it is evidently missing what they know.
    fn automatically(&self, tool: &str, action: &Action) -> Decision {
        if matches!(action, Action::Read { .. }) || self.an_edit_here(action) {
            return Self::once();
        }
        let Some(advice) = self.judge.judge(tool, action) else {
            return self.person.ask(tool, action);
        };
        if advice.safe {
            self.standing.in_a_row.store(0, Ordering::Relaxed);
            return Self::once();
        }
        self.standing.in_a_row.fetch_add(1, Ordering::Relaxed);
        self.standing.in_all.fetch_add(1, Ordering::Relaxed);
        if self.standing.spent() {
            (self.notice)(format!(
                "auto mode has refused {IN_A_ROW} in a row or {IN_ALL} in all, so you are asked \
                 instead; allowing this one takes it up again"
            ));
            let decision = self.person.ask_advised(tool, action, Some(&advice));
            if matches!(decision, Decision::Allow { .. }) {
                self.standing.in_a_row.store(0, Ordering::Relaxed);
                self.standing.in_all.store(0, Ordering::Relaxed);
            }
            return decision;
        }
        (self.notice)(format!(
            "auto mode refused {} {} [{}]: {}",
            action.verb(),
            action.subject(),
            advice.rule,
            advice.reason
        ));
        self.refuse(
            action,
            format!(
                "Refused by the safety check [{}]: {} Do it another way, or leave it for the \
                 person to do; asking again as it is will be refused again.",
                advice.rule, advice.reason
            ),
        )
    }
}

fn key(action: &Action) -> String {
    format!("{} {}", action.verb(), action.subject())
}

impl magi_tools::approve::Approver for Judged {
    fn ask(&self, tool: &str, action: &Action) -> Decision {
        if let Some(rule) = self.rules.deny.iter().find(|rule| rule.names(action)) {
            return self.refuse(
                action,
                format!(
                    "`magi.deny` forbids it ({} {:?}), in every mode. It is not to be done another \
                     way either.",
                    rule.verb, rule.scope
                ),
            );
        }
        if self.rules.ask.iter().any(|rule| rule.names(action)) {
            let advice = self.judge.judge(tool, action);
            return self.person.ask_advised(tool, action, advice.as_ref());
        }
        match self.standing.mode() {
            Mode::Locked => self.refuse(
                action,
                "This session is locked: what no rule allows is refused rather than asked about. \
                 Carry on with what is allowed."
                    .to_owned(),
            ),
            Mode::Auto => self.automatically(tool, action),
            Mode::Edits if self.an_edit_here(action) => Self::once(),
            // The person decides, with the second model's view beside the question when there is
            // one: a long command is read for them, not decided for them.
            Mode::Ask | Mode::Edits => {
                let advice = self.judge.judge(tool, action);
                self.person.ask_advised(tool, action, advice.as_ref())
            }
        }
    }

    fn why(&self, action: &Action) -> Option<String> {
        self.refused.lock().ok()?.get(&key(action)).cloned()
    }

    fn overrides(&self, action: &Action) -> bool {
        self.rules
            .deny
            .iter()
            .chain(&self.rules.ask)
            .any(|rule| rule.names(action))
    }
}

/// The second model, reached the way a surface reaches a helper: a question down the session's
/// own channel, waited for on this thread.
pub struct Helper {
    asking: tokio::sync::mpsc::UnboundedSender<crate::knowing::Wondering>,
    cwd: String,
}

/// How long a judgement may take. The turn is waiting on it, and past this the person is asked.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

impl Helper {
    #[must_use]
    pub fn new(
        asking: tokio::sync::mpsc::UnboundedSender<crate::knowing::Wondering>,
        cwd: &std::path::Path,
    ) -> Self {
        Self {
            asking,
            cwd: cwd.display().to_string(),
        }
    }
}

impl Judge for Helper {
    fn judge(&self, tool: &str, action: &Action) -> Option<Advice> {
        let (back, answer) = std::sync::mpsc::channel();
        let input = format!(
            "The project directory: {}\nThe tool: {tool}\nThe action: {} {}",
            self.cwd,
            action.verb(),
            action.subject()
        );
        self.asking
            .send(crate::knowing::Wondering {
                wonder: magi_proto::wondering::Wonder::Helper,
                args: serde_json::json!({
                    "role": "safety", "instruction": INSTRUCTION, "input": input, "said": true,
                    "max_tokens": 400, "timeout_ms": PATIENCE.as_millis() as u64,
                    "schema": { "type": "object", "required": ["safe", "rule", "reason"],
                        "properties": { "safe": { "type": "boolean" },
                            "rule": { "type": "string" }, "reason": { "type": "string" } } },
                }),
                back,
            })
            .ok()?;
        match answer.recv_timeout(PATIENCE).ok()? {
            magi_proto::wondering::Answered::Told { said } => read(said["text"].as_str()?),
            magi_proto::wondering::Answered::Refused { .. } => None,
        }
    }
}

/// A verdict out of what a model wrote, which is JSON with or without a fence round it.
fn read(text: &str) -> Option<Advice> {
    let from = text.find('{')?;
    let to = text.rfind('}')?;
    serde_json::from_str(text.get(from..=to)?).ok()
}

/// Change who is asked, for the gate and for what every screen is shown. `next` cycles.
///
/// # Errors
/// When `named` is no mode.
pub async fn switch(
    session: &tokio::sync::Mutex<crate::session::Session>,
    standing: &Standing,
    named: &str,
) -> Result<(), String> {
    let mut held = session.lock().await;
    let mode = if named.trim() == "next" {
        held.mode.next()
    } else {
        Mode::named(named).ok_or_else(|| {
            format!("`{named}` is no mode: ask, edits, auto or locked, or `next` to cycle")
        })?
    };
    held.mode = mode;
    standing.set(mode);
    let cursor = held.cursor();
    let _ = held
        .publisher()
        .send(magi_proto::HarnessEvent::ModeChanged { cursor, mode });
    Ok(())
}

#[cfg(test)]
mod tests;
