//! Talking to atom, the layer that knows about the other agents on this machine.
//!
//! atom is a separate program, in a separate repository, that knows nothing about axon. It owns
//! naming, the socket other sessions reach this one at, the walls between them, and the
//! vocabulary a model calls. axon owns turns, a transcript, a model and a screen. Neither links
//! the other.
//!
//! This file is the whole of what axon knows about it: spawn it, read what it says, tell it what
//! this session is doing. Two things cross, one JSON object per line:
//!
//! ```text
//! ->  {"say":"doing","busy":true,"working_for":7,"waiting":0}
//! <-  {"heard":"listening","at":"…/atom/axon/psi-omicron","as":"axon/main/psi-omicron"}
//! <-  {"heard":"message","who":"axon/main/beta-nu","sort":"attention","text":"…"}
//! ```
//!
//! # atom being absent is the ordinary case
//!
//! Exactly as aeon's is. A session with no atom has no siblings, no name beyond its project, and
//! no `agent` tool — and is otherwise a working session. Nothing here returns an error for it,
//! because "you have not installed the other program" is not a thing to fail a session over.
//!
//! # Why it names this session
//!
//! Because it can see the namespace and axon cannot. Two sessions started in the same second
//! draw the same clock, and the collision surfaces only as a failed bind — after each has told
//! everyone what it is called. atom holds the directory, so atom looks first and says `as`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

/// What the layer reads `axon.agent_talk` out of.
///
/// atom's name for it, spelled here because axon is what sets it — on the `serve` child and in
/// the environment tools are spawned from, since those are two processes and one setting.
pub const TALK: &str = "ATOM_TALK";

/// What atom says, one JSON object per line on its stdout.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "heard", rename_all = "lowercase")]
pub enum Heard {
    /// The socket is bound, and this is the name it was bound under.
    Listening {
        /// Where it is listening.
        at: String,
        /// Who this session is, as `project/role/id`.
        #[serde(rename = "as")]
        named: String,
    },
    /// A message arrived from another session.
    Message {
        /// Who sent it.
        who: String,
        /// What sort it is: `note`, `question`, `attention`…
        sort: String,
        /// What they said.
        text: String,
        // atom also says which message this answers. Not taken, because axon has nowhere to put
        // it: message ids are the layer's and never reach a transcript, so a model that wants
        // the thread asks `agent --verb inbox`, which has them. Serde drops what is not named
        // here, so this is a field axon does not read rather than a wire it cannot parse.
    },
    /// Who else is in this project, whenever that changes.
    ///
    /// Pushed, because what wants it is the `$` popup: a completion offered on a keystroke
    /// cannot spawn a process or open a socket to answer, and axon reading the directory itself
    /// would be a second place that knows where sockets live.
    Around {
        /// Every session listening, by id, this one included.
        names: Vec<String>,
    },
    /// Somebody with the right to stop this session did.
    Stopped,
}

/// A running atom, and the pipe back to it.
pub struct Atom {
    child: Child,
    told: Option<ChildStdin>,
    /// The program this one was started from, for the one-shot calls that go beside the pipe.
    program: String,
    /// What this session ended up being called.
    pub named: String,
}

impl Atom {
    /// Start atom for a session in `project`, and wait until it is reachable.
    ///
    /// Waited for on purpose. The name comes back on that first line and this session needs it
    /// before it can journal anything or draw its own footer — and a socket announced before it
    /// is bound is a session that meets itself as "nothing is listening".
    ///
    /// `talk` is `axon.agent_talk`, handed over rather than interpreted: how far a session may
    /// reach is the layer's question, and axon knowing what the levels are called would be two
    /// programs holding one answer.
    ///
    /// `None` when atom is not installed or will not start, which is a session without siblings
    /// rather than a failure.
    pub fn start(
        program: &str,
        project: &str,
        talk: Option<&str>,
    ) -> Option<(Self, std::path::PathBuf)> {
        let mut child = Command::new(program)
            .arg("serve")
            .arg("--project")
            .arg(project)
            .envs(talk.map(|talk| (TALK.to_owned(), talk.to_owned())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Its own complaints go where this session's do. Swallowed, a refusal to bind would
            // present as "no siblings" and there would be nothing to read about why.
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;

        let stdout = child.stdout.take()?;
        let mut reading = BufReader::new(stdout);
        let mut first = String::new();
        // Blocking, and briefly: atom binds and answers before it does anything else.
        if reading.read_line(&mut first).ok()? == 0 {
            let _ = child.kill();
            return None;
        }
        let Ok(Heard::Listening { at, named }) = serde_json::from_str::<Heard>(&first) else {
            let _ = child.kill();
            return None;
        };

        let told = child.stdin.take();
        Some((
            Self {
                child,
                told,
                program: program.to_owned(),
                named,
            },
            std::path::PathBuf::from(at),
        ))
    }

    /// What to tell the model about the sessions this prompt named.
    ///
    /// Asked of the atom this session already started, so the answer comes from the same
    /// program that named it — see [`briefing`] for why it is a spawn rather than a message.
    #[must_use]
    pub fn briefing(&self, text: &str, project: &str) -> String {
        briefing(&self.program, text, project)
    }

    /// The stdout to read arrivals from, taken once.
    pub fn hearing(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }

    /// Tell atom what this session is doing.
    ///
    /// So `status` answers truthfully rather than plausibly: atom cannot see a turn running, and
    /// a sibling deciding whether to interrupt is asking exactly that.
    pub fn doing(&mut self, busy: bool, working_for: u64, waiting: usize) {
        let Some(told) = self.told.as_mut() else {
            return;
        };
        let line = serde_json::json!({
            "say": "doing",
            "busy": busy,
            "working_for": working_for,
            "waiting": waiting,
        });
        // Best effort. atom having gone away is a session without siblings, and taking the UI
        // down over it would be the tail wagging the dog.
        let _ = writeln!(told, "{line}");
        let _ = told.flush();
    }
}

impl Drop for Atom {
    /// Let go of the pipe, which is how atom knows the session is over.
    ///
    /// Dropping the stdin is the whole signal — atom reads its parent's pipe and exits when it
    /// closes — so this is closing a handle, not killing anything. The `wait` is what keeps it
    /// off the process table until the shell reaps it.
    fn drop(&mut self) {
        self.told.take();
        let _ = self.child.wait();
    }
}

/// What to tell the model about the sessions a prompt named.
///
/// The scan is axon's: a prompt, a cursor and a table of sigils are all things only a harness
/// has. What is *known* about a name is atom's, so this hands one to the other — over argv,
/// because it is a question with an answer and nothing to hold open.
///
/// Empty when the prompt named nobody, which is almost every prompt, and empty when atom is not
/// installed. Both are the same thing to a caller: nothing to add.
#[must_use]
pub fn briefing(program: &str, text: &str, project: &str) -> String {
    let named = axon_tui::trigger::named(text, axon_tui::trigger::Trigger::Instance);
    if named.is_empty() {
        return String::new();
    }
    let mut command = Command::new(program);
    command.arg("brief").arg("--project").arg(project);
    for name in &named {
        command.arg("--name").arg(name);
    }
    command
        .output()
        .ok()
        .filter(|done| done.status.success())
        .map(|done| String::from_utf8_lossy(&done.stdout).into_owned())
        .unwrap_or_default()
}

/// What axon knows about atom, and what it does not.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_atom_is_a_session_without_siblings() {
        // The aeon rule: a sibling not being installed is the ordinary case, not a failure.
        // This is the one that decides whether somebody with no atom can use axon at all.
        assert!(Atom::start("atom-that-is-not-installed", "axon", None).is_none());
    }

    #[test]
    fn a_prompt_naming_nobody_asks_atom_nothing() {
        // Not merely empty — it must not *run* anything. A process per prompt, for a prompt that
        // named no instances, would be a spawn on every keystroke's worth of work.
        assert!(briefing("atom-that-is-not-installed", "fix the parser", "axon").is_empty());
    }

    #[test]
    fn a_briefing_from_a_missing_atom_is_empty_rather_than_an_error() {
        assert!(briefing("atom-that-is-not-installed", "ask $beta-nu", "axon").is_empty());
    }

    #[test]
    fn what_atom_says_is_read_as_what_it_means() {
        // The wire between two repositories, and the only place axon knows its shape.
        let listening: Heard = serde_json::from_str(
            r#"{"heard":"listening","at":"/run/atom/axon/psi-omicron","as":"axon/main/psi-omicron"}"#,
        )
        .expect("reads");
        let Heard::Listening { at, named } = listening else {
            panic!("not a listening line");
        };
        assert_eq!(named, "axon/main/psi-omicron");
        assert!(at.ends_with("psi-omicron"));

        let arrived: Heard = serde_json::from_str(
            r#"{"heard":"message","who":"axon/main/beta-nu","sort":"attention","text":"look"}"#,
        )
        .expect("reads");
        let Heard::Message { who, sort, .. } = arrived else {
            panic!("not a message");
        };
        assert_eq!(who, "axon/main/beta-nu");
        assert_eq!(sort, "attention");
    }

    #[test]
    fn a_line_from_a_newer_atom_is_not_read_as_something_it_is_not() {
        // Two repositories move apart. A `heard` this build has never seen should fail to parse
        // rather than land in the nearest arm — an unknown line read as a `message` would put
        // something in the transcript that nobody said.
        assert!(serde_json::from_str::<Heard>(r#"{"heard":"whistling","tune":"…"}"#).is_err());
    }
}
