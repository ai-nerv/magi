//! Talking to melchior, the layer that knows about the other agents on this machine.
//!
//! melchior is a separate program, in a separate repository, that knows nothing about magi. It owns
//! naming, the socket other sessions reach this one at, the walls between them, and the
//! vocabulary a model calls. magi owns turns, a transcript, a model and a screen. Neither links
//! the other.
//!
//! This file is the whole of what magi knows about it: spawn it, read what it says, tell it what
//! this session is doing. Two things cross, one JSON object per line:
//!
//! ```text
//! ->  {"event":"doing","busy":true,"working_for":7,"waiting":0}
//! <-  {"event":"listening","at":"…/melchior/magi/psi-omicron","as":"magi/main/psi-omicron"}
//! <-  {"event":"message","who":"magi/main/beta-nu","sort":"attention","text":"…"}
//! ```
//!
//! # melchior being absent is the ordinary case
//!
//! Exactly as balthasar's is. A session with no melchior has no siblings, no name beyond its project, and
//! no `agent` tool — and is otherwise a working session. Nothing here returns an error for it,
//! because "you have not installed the other program" is not a thing to fail a session over.
//!
//! # Why it names this session
//!
//! Because it can see the namespace and magi cannot. Two sessions started in the same second
//! draw the same clock, and the collision surfaces only as a failed bind — after each has told
//! everyone what it is called. melchior holds the directory, so melchior looks first and says `as`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

/// What the layer reads `magi.agent_talk` out of.
///
/// melchior's name for it, spelled here because magi is what sets it — on the `serve` child and in
/// the environment tools are spawned from, since those are two processes and one setting.
pub const TALK: &str = "MAGI_MELCHIOR_TALK";

/// What a session learns its *run* from: the id of the root that started the whole tree.
///
/// melchior's name again. It is set by whoever minted this session, inherited unchanged however
/// deep the tree goes, and absent for a session somebody started at a terminal — which is a root,
/// and a root's run is its own id. See [`run_of`].
pub const SESSION: &str = "MAGI_MELCHIOR_SESSION";

/// Which run this session belongs to.
///
/// **The same rule melchior applies to itself**, which is what makes the two agree: `serve` reads
/// this variable out of the environment magi spawned it with and writes the answer to
/// `<project>/<id>.session`, treating a session with no note as its own root. Working it out here
/// a second way would give magi and the directory two different names for one run.
///
/// `None` for a session with no melchior, which has no id and therefore no run to belong to.
///
/// `named` is `project/role/id` as melchior gave it.
#[must_use]
pub fn run_of(named: &str) -> Option<String> {
    let minted = std::env::var(SESSION).ok();
    run_from(minted.as_deref(), named)
}

/// The same answer, with the environment handed in rather than read.
///
/// Split out so the two cases can be checked without setting a process-wide variable.
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
    /// The socket is bound, and this is the name it was bound under.
    Listening {
        /// Where it is listening.
        at: String,
        /// Who this session is, as `project/role/id`.
        #[serde(rename = "as")]
        named: String,
        /// Which run it belongs to, as the note beside its socket says.
        ///
        /// Defaulted rather than required, so an older melchior — which says nothing here — still
        /// names a session rather than failing to parse the line that starts one. What magi does
        /// without it is [`run_of`].
        #[serde(default)]
        run: String,
    },
    /// A message arrived from another session.
    Message {
        /// Who sent it.
        who: String,
        /// What sort it is: `note`, `question`, `attention`…
        sort: String,
        /// What they said.
        text: String,
        // melchior also says which message this answers. Not taken, because magi has nowhere to put
        // it: message ids are the layer's and never reach a transcript, so a model that wants
        // the thread asks `agent --verb inbox`, which has them. Serde drops what is not named
        // here, so this is a field magi does not read rather than a wire it cannot parse.
    },
    /// Who else is in this project, whenever that changes.
    ///
    /// Pushed, because what wants it is the `$` popup: a completion offered on a keystroke
    /// cannot spawn a process or open a socket to answer, and magi reading the directory itself
    /// would be a second place that knows where sockets live.
    Around {
        /// Every session listening, this one included.
        ///
        /// Defaulted, because the melchior that said `names` here is still installed on
        /// machines this build runs on and there is no reason it should not be — see [`peers`],
        /// which is what magi does with either.
        #[serde(default)]
        agents: Vec<Peer>,
        /// What a melchior too old to say `agents` sends instead: bare ids and nothing else.
        #[serde(default)]
        names: Vec<String>,
    },
    /// Somebody with the right to stop this session did.
    Stopped,
    /// A session this one asked to be taken on by has accepted.
    ///
    /// Its own line rather than the message that also arrives, because they have different
    /// readers. The message is for the model — somebody said yes, here is who. This is for the
    /// harness, and carries what that session lent: permissions written into a transcript are
    /// permissions a model can read and reason about acquiring more of.
    Adopted {
        /// Who took this session on, as `project/role/id`.
        by: String,
        /// What they handed over, as this side wrote it.
        #[serde(default)]
        handover: Option<String>,
    },
    /// Another session is asking to become this one's child, and a person has to answer.
    ///
    /// Up the pipe rather than into the transcript, because it is not the model's to answer. It
    /// decides whether another session may act with this one's authority, and a model that could
    /// accept on its own behalf would be granting itself a second pair of hands.
    Asked {
        /// The request, quoted back when it is answered.
        id: String,
        /// Who is asking, as `project/role/id`.
        who: String,
        /// Why, in their words — the whole of what the person has to go on.
        why: String,
    },
}

/// One session melchior says is listening, and how to find the screen it draws on.
///
/// The id is what addresses it and what `$` completes to. The other two are what a bare name
/// could never tell you: what that agent is *for*, which a person reads off a list, and where
/// its harness draws it, which is the one fact this side of the family could not work out for
/// itself — a host socket is named after a key its own process keeps private, so the project's
/// socket directory is a heap of live sessions with nothing saying which is which.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Peer {
    /// Its id: the third part of `project/role/id`.
    pub id: String,
    /// What it says it is for, one word. Empty from a melchior too old to say.
    #[serde(default)]
    pub role: String,
    /// The socket its harness draws over, or `None` for an agent that published none.
    ///
    /// A path, not a promise. Nothing has dialled it, and the session that published it may
    /// have gone since — which is the same thing that is true of every name in this list, and
    /// is found out the same way, on the first call.
    #[serde(default)]
    pub ui: Option<std::path::PathBuf>,
}

/// Everyone melchior named, whichever of the two ways it said it.
///
/// **A magi meeting an older melchior still has peers.** The two programs are released apart,
/// so a build of each is going to meet a build of the other that predates it — and the whole of
/// what the older one can say is a list of ids. Read strictly, that is a `$` popup that offers
/// nobody for as long as the session runs, and nothing anywhere saying why.
///
/// A name with no role and no screen is exactly what magi had before this existed, so the
/// fallback loses nothing that was ever there.
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

/// What a session says it is for, when whoever started it knows.
///
/// **Taken whole or not at all.** melchior resolves a role from four places — these flags, the
/// environment a fork was minted with, a config, then `main` — and whichever speaks first is
/// taken entirely, because a name out of one and a sentence out of another describes a role
/// nobody declared. Carrying both in one value is what keeps magi from being the place that
/// splits them.
///
/// Both `None` for the ordinary session, which is named by whoever minted it or is `main`. It is
/// the headless one that has no other source: nobody minted it, so the only thing that can say
/// what it is for is the command line that started it — see [`crate::child`].
#[derive(Default, Clone, Copy)]
pub struct Role<'a> {
    /// The one word a coordinator routes by.
    pub name: Option<&'a str>,
    /// What that word means, in a sentence.
    pub description: Option<&'a str>,
}

/// A running melchior, and the pipe back to it.
pub struct Melchior {
    child: Child,
    told: Option<ChildStdin>,
    /// What melchior is saying, from the line after the one that named this session.
    ///
    /// **The reader, not the pipe.** Reading the first line needs a buffer, and a buffer holds
    /// whatever came after the newline it stopped at — so handing back the raw `ChildStdout`
    /// and making the caller wrap it again drops however much of the next message was already
    /// in there. It was worse than that: taking the pipe out of the child to read one line and
    /// then letting the reader fall out of scope *closed* it, and the session heard the line
    /// that named it and then nothing, for as long as it ran.
    hears: Option<BufReader<std::process::ChildStdout>>,
    /// The program this one was started from, for the one-shot calls that go beside the pipe.
    program: String,
    /// What this session ended up being called.
    pub named: String,
    /// Which run it belongs to, as melchior's own note says. Empty from a melchior too old to
    /// say — see [`run_of`], which is what magi falls back to.
    pub run: String,
}

/// Exactly how the layer is started: what is on its argv, and what is in its environment.
///
/// Split from [`Melchior::start`] so a test can read a real one back rather than write out a
/// second copy of the same decisions — the arrangement [`crate::forking::spawning`] is under, and
/// for the same reason. The copy is what goes on passing after somebody changes the spawn.
///
/// **`--ui` is unconditional and that is the load-bearing part.** It is the only way a peer can
/// learn where this session draws, and a session that withheld it because it had no terminal of
/// its own would be exactly the agent nobody can attach to — which is the one failure a headless
/// magi has no way to report, because it has nowhere to report it.
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
    /// Start melchior for a session in `project`, and wait until it is reachable.
    ///
    /// Waited for on purpose. The name comes back on that first line and this session needs it
    /// before it can journal anything or draw its own footer — and a socket announced before it
    /// is bound is a session that meets itself as "nothing is listening".
    ///
    /// `talk` is `magi.agent_talk`, handed over rather than interpreted: how far a session may
    /// reach is the layer's question, and magi knowing what the levels are called would be two
    /// programs holding one answer.
    ///
    /// `ui` is the socket this session will bind its own UI on, published so a sibling can find
    /// the screen rather than only the name. **Handed in, not looked up**: it is magi's own
    /// path, made from a key nothing outside this process shares, and melchior has no way to
    /// arrive at it. Passing it here rather than telling melchior later is what makes the note
    /// and the roster agree from the first tick — a session announced without one is on every
    /// peer's list as an agent with no screen until it says otherwise, and nothing would.
    ///
    /// `role` is what this session is for, when the command line said. See [`Role`].
    ///
    /// `None` when melchior is not installed or will not start, which is a session without siblings
    /// rather than a failure.
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
            // Its own complaints go where this session's do. Swallowed, a refusal to bind would
            // present as "no siblings" and there would be nothing to read about why.
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

    /// What to tell the model about the sessions this prompt named.
    ///
    /// Asked of the melchior this session already started, so the answer comes from the same
    /// program that named it — see [`briefing`] for why it is a spawn rather than a message.
    #[must_use]
    pub fn briefing(&self, text: &str, project: &str) -> String {
        briefing(&self.program, text, project)
    }

    /// What melchior is saying, taken once, for a thread to read to the end of.
    ///
    /// Taken rather than borrowed because reading it blocks, and everything else here happens on
    /// the frame loop.
    pub fn hearing(&mut self) -> Option<BufReader<std::process::ChildStdout>> {
        self.hears.take()
    }

    /// Tell melchior what this session is doing.
    ///
    /// So `status` answers truthfully rather than plausibly: melchior cannot see a turn running, and
    /// a sibling deciding whether to interrupt is asking exactly that.
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
        // Best effort. melchior having gone away is a session without siblings, and taking the UI
        // down over it would be the tail wagging the dog.
        let _ = writeln!(told, "{line}");
        let _ = told.flush();
    }

    /// Say what the person decided about a request this session was asked to answer.
    ///
    /// Sent whichever way they answered. A refusal that went back as silence is one the asking
    /// session cannot tell from an answer that never came, so it would wait for good — and the
    /// person who said no would have no way to know it had not landed.
    /// `lending` is what this session hands the one it has taken on. melchior carries it unread and
    /// delivers it to the other harness — it is magi's idea, not the layer's, and a layer that
    /// understood permissions would be a second place to change when they change.
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
        let _ = writeln!(told, "{line}");
        let _ = told.flush();
    }
}

impl Drop for Melchior {
    /// Let go of the pipe, which is how melchior knows the session is over.
    ///
    /// Dropping the stdin is the whole signal — melchior reads its parent's pipe and exits when it
    /// closes — so this is closing a handle, not killing anything. The `wait` is what keeps it
    /// off the process table until the shell reaps it.
    fn drop(&mut self) {
        self.told.take();
        let _ = self.child.wait();
    }
}

/// What to tell the model about the sessions a prompt named.
///
/// The scan is magi's: a prompt, a cursor and a table of sigils are all things only a harness
/// has. What is *known* about a name is melchior's, so this hands one to the other — over argv,
/// because it is a question with an answer and nothing to hold open.
///
/// Empty when the prompt named nobody, which is almost every prompt, and empty when melchior is not
/// installed. Both are the same thing to a caller: nothing to add.
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
