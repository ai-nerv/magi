//! Talking to melchior, the layer that knows about the other agents on this machine: a separate
//! program owning naming, sockets and the walls between sessions, one JSON object per line. Absent
//! is the ordinary case — no siblings, no `agent` tool, and otherwise a working session.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

/// What the layer reads `magi.agent_talk` out of, spelled here because magi is what sets it.
pub const TALK: &str = "MAGI_MELCHIOR_TALK";

/// The id of the root that started the whole tree: inherited unchanged, absent for a root itself.
pub const SESSION: &str = "MAGI_MELCHIOR_SESSION";

/// Which run this session belongs to, by the same rule melchior applies to itself.
#[must_use]
pub fn run_of(named: &str) -> Option<String> {
    let minted = std::env::var(SESSION).ok();
    run_from(minted.as_deref(), named)
}

fn run_from(minted: Option<&str>, named: &str) -> Option<String> {
    minted
        .map(str::trim)
        .filter(|run| !run.is_empty())
        .or_else(|| {
            named
                .split('/')
                .nth(2)
                .map(str::trim)
                .filter(|id| !id.is_empty())
        })
        .map(ToOwned::to_owned)
}

/// What melchior says, one JSON object per line on its stdout.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Heard {
    Listening {
        at: String,
        #[serde(rename = "as")]
        named: String,
        /// Defaulted, so an older melchior still names a session; see [`run_of`].
        #[serde(default)]
        run: String,
    },
    Message {
        who: String,
        sort: String,
        text: String,
        // The message id melchior also sends is not taken: a model wanting the thread asks
        // `agent --verb inbox`.
    },
    /// Pushed, because the `$` popup cannot spawn a process or open a socket on a keystroke.
    Around {
        /// Defaulted, because an older melchior says `names` here instead — see [`peers`].
        #[serde(default)]
        agents: Vec<Peer>,
        /// What a melchior too old to say `agents` sends instead: bare ids and nothing else.
        #[serde(default)]
        names: Vec<String>,
    },
    Stopped,
    /// A session this one asked to be taken on by has accepted; carries what that session lent.
    Adopted {
        by: String,
        #[serde(default)]
        handover: Option<String>,
    },
    /// Another session asking to become this one's child. Up the pipe rather than into the
    /// transcript: a model that could accept would be granting itself a second pair of hands.
    Asked {
        id: String,
        who: String,
        why: String,
    },
}

// The message id melchior also sends is not taken; a model wanting the thread asks inbox.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Peer {
    pub id: String,
    #[serde(default)]
    pub role: String,
    /// The socket its harness draws over. A path, not a promise: that session may have gone.
    #[serde(default)]
    pub ui: Option<std::path::PathBuf>,
}

/// Everyone melchior named, either way it said it: an older melchior can only say a list of ids.
#[must_use]
pub fn peers(agents: Vec<Peer>, names: Vec<String>) -> Vec<Peer> {
    if !agents.is_empty() {
        return agents;
    }
    names
        .into_iter()
        .map(|id| Peer {
            id,
            role: String::new(),
            ui: None,
        })
        .collect()
}

/// Another session asking to become this one's child. Up the pipe rather than into the
/// transcript: a model that could accept would grant itself a second pair of hands.
#[derive(Default, Clone, Copy)]
pub struct Role<'a> {
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
}

/// A running melchior, and the pipe back to it.
pub struct Melchior {
    child: Child,
    told: Option<ChildStdin>,
    /// One session melchior says is listening, and where it draws: a host socket is named after a key
    /// its own process keeps private.
    hears: Option<BufReader<std::process::ChildStdout>>,
    program: String,
    pub named: String,
    /// Empty from a melchior too old to say — see [`run_of`], which is what magi falls back to.
    pub run: String,
}

/// The reader, not the pipe: dropping it closes the pipe, so the session hears its own name
/// and then nothing.
fn serving(
    program: &str,
    project: &str,
    talk: Option<&str>,
    ui: &std::path::Path,
    role: Role<'_>,
) -> Command {
    let mut serving = Command::new(program);
    serving
        .arg("serve")
        .arg("--project")
        .arg(project)
        .arg("--ui")
        .arg(ui)
        .envs(talk.map(|talk| (TALK.to_owned(), talk.to_owned())));
    if let Some(name) = role.name {
        serving.arg("--role").arg(name);
    }
    if let Some(said) = role.description {
        serving.arg("--role-description").arg(said);
    }
    serving
}

impl Melchior {
    /// Start melchior for a session in `project` and wait until reachable: the name comes back
    /// on that first line, and a socket announced before it is bound meets itself as "nothing is
    /// listening". `ui` is handed in, not looked up, so the note and the roster agree at once.
    pub fn start(
        program: &str,
        project: &str,
        talk: Option<&str>,
        ui: &std::path::Path,
        role: Role<'_>,
    ) -> Option<(Self, std::path::PathBuf)> {
        let mut child = serving(program, project, talk, ui, role)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Swallowed, a refusal to bind would present as "no siblings" with nothing to read.
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;

        let stdout = child.stdout.take()?;
        let mut reading = BufReader::new(stdout);
        let mut first = String::new();
        // Blocking, and briefly: melchior binds and answers before it does anything else.
        if reading.read_line(&mut first).ok()? == 0 {
            let _ = child.kill();
            return None;
        }
        let Ok(Heard::Listening { at, named, run }) = serde_json::from_str::<Heard>(&first) else {
            let _ = child.kill();
            return None;
        };

        let told = child.stdin.take();
        Some((
            Self {
                child,
                told,
                hears: Some(reading),
                program: program.to_owned(),
                named,
                run,
            },
            std::path::PathBuf::from(at),
        ))
    }

    /// What to tell the model about the sessions this prompt named, asked of this session's melchior.
    #[must_use]
    pub fn briefing(&self, text: &str, project: &str) -> String {
        briefing(&self.program, text, project)
    }

    /// Taken rather than borrowed because reading it blocks, and the rest here is on the frame loop.
    pub fn hearing(&mut self) -> Option<BufReader<std::process::ChildStdout>> {
        self.hears.take()
    }

    /// Tell melchior what this session is doing, so `status` answers truthfully: it cannot see a turn.
    pub fn doing(&mut self, busy: bool, working_for: u64, waiting: usize) {
        let Some(told) = self.told.as_mut() else {
            return;
        };
        let line = serde_json::json!({
            "event": "doing",
            "busy": busy,
            "working_for": working_for,
            "waiting": waiting,
        });
        // Best effort: melchior having gone away is a session without siblings.
        let _ = writeln!(told, "{line}");
        let _ = told.flush();
    }

    /// Say what the person decided about a request this session was asked to answer, sent whichever way
    /// they answered: silence leaves the asking session waiting for good. `lending` is carried unread.
    pub fn answered(
        &mut self,
        id: &str,
        accept: bool,
        lending: Option<&[magi_proto::permit::Grant]>,
    ) {
        let Some(told) = self.told.as_mut() else {
            return;
        };
        let line = serde_json::json!({
            "event": "answered",
            "id": id,
            "accept": accept,
            "handover": lending.and_then(|grants| serde_json::to_string(grants).ok()),
        });
        // Unlike `doing`, this is sent once and cannot retry, so a failure gets a log line.
        if let Err(why) = writeln!(told, "{line}").and_then(|()| told.flush()) {
            magi_model::noted!("melchior: the answer to {id} did not reach the layer: {why}");
        }
    }
}

impl Drop for Melchior {
    /// Let go of the pipe: melchior reads its parent's pipe and exits when it closes.
    fn drop(&mut self) {
        self.told.take();
        let _ = self.child.wait();
    }
}

/// What to tell the model about the sessions a prompt named, over argv: a question with an answer.
#[must_use]
pub fn briefing(program: &str, text: &str, project: &str) -> String {
    let named = magi_tui::trigger::named(text, magi_tui::trigger::Trigger::Instance);
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

/// What magi knows about melchior, and what it does not.
#[cfg(test)]
#[path = "melchior/tests.rs"]
mod tests;
