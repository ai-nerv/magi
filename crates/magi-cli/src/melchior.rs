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
//! ->  {"say":"doing","busy":true,"working_for":7,"waiting":0}
//! <-  {"heard":"listening","at":"…/melchior/magi/psi-omicron","as":"magi/main/psi-omicron"}
//! <-  {"heard":"message","who":"magi/main/beta-nu","sort":"attention","text":"…"}
//! ```
//!
//! # melchior being absent is the ordinary case
//!
//! Exactly as aeon's is. A session with no melchior has no siblings, no name beyond its project, and
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

/// What melchior says, one JSON object per line on its stdout.
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
        /// Every session listening, by id, this one included.
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
    /// `None` when melchior is not installed or will not start, which is a session without siblings
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
        // Blocking, and briefly: melchior binds and answers before it does anything else.
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
                hears: Some(reading),
                program: program.to_owned(),
                named,
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
            "say": "doing",
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
            "say": "answered",
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
mod tests {
    use super::*;

    #[test]
    fn a_missing_melchior_is_a_session_without_siblings() {
        // The aeon rule: a sibling not being installed is the ordinary case, not a failure.
        // This is the one that decides whether somebody with no melchior can use magi at all.
        assert!(Melchior::start("melchior-that-is-not-installed", "magi", None).is_none());
    }

    #[test]
    fn a_prompt_naming_nobody_asks_melchior_nothing() {
        // Not merely empty — it must not *run* anything. A process per prompt, for a prompt that
        // named no instances, would be a spawn on every keystroke's worth of work.
        assert!(briefing("melchior-that-is-not-installed", "fix the parser", "magi").is_empty());
    }

    #[test]
    fn a_briefing_from_a_missing_melchior_is_empty_rather_than_an_error() {
        assert!(briefing("melchior-that-is-not-installed", "ask $beta-nu", "magi").is_empty());
    }

    #[test]
    fn what_melchior_says_is_read_as_what_it_means() {
        // The wire between two repositories, and the only place magi knows its shape.
        let listening: Heard = serde_json::from_str(
            r#"{"heard":"listening","at":"/run/melchior/magi/psi-omicron","as":"magi/main/psi-omicron"}"#,
        )
        .expect("reads");
        let Heard::Listening { at, named } = listening else {
            panic!("not a listening line");
        };
        assert_eq!(named, "magi/main/psi-omicron");
        assert!(at.ends_with("psi-omicron"));

        let arrived: Heard = serde_json::from_str(
            r#"{"heard":"message","who":"magi/main/beta-nu","sort":"attention","text":"look"}"#,
        )
        .expect("reads");
        let Heard::Message { who, sort, .. } = arrived else {
            panic!("not a message");
        };
        assert_eq!(who, "magi/main/beta-nu");
        assert_eq!(sort, "attention");
    }

    #[test]
    fn a_line_from_a_newer_melchior_is_not_read_as_something_it_is_not() {
        // Two repositories move apart. A `heard` this build has never seen should fail to parse
        // rather than land in the nearest arm — an unknown line read as a `message` would put
        // something in the transcript that nobody said.
        assert!(serde_json::from_str::<Heard>(r#"{"heard":"whistling","tune":"…"}"#).is_err());
    }

    /// A second session in the same project, so there is somebody to be talked to.
    ///
    /// A bare child rather than another [`Melchior`], on purpose: if the thing under test is broken,
    /// the fixture must not be broken the same way.
    fn a_sibling(project: &str) -> Option<(std::process::Child, String)> {
        let mut child = Command::new("melchior")
            .args(["serve", "--project", project])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut said = String::new();
        BufReader::new(child.stdout.as_mut()?)
            .read_line(&mut said)
            .ok()?;
        let named = said
            .split("\"as\":\"")
            .nth(1)?
            .split('"')
            .next()?
            .to_owned();
        Some((child, named))
    }

    /// The three that tell a spawned process which session it is speaking as.
    fn as_session(command: &mut Command, named: &str) {
        let mut parts = named.split('/');
        command
            .env("MAGI_MELCHIOR_PROJECT", parts.next().unwrap_or_default())
            .env("MAGI_MELCHIOR_ROLE", parts.next().unwrap_or_default())
            .env("MAGI_MELCHIOR_ID", parts.next().unwrap_or_default());
    }

    /// The last segment, which is what a sibling is addressed by inside one project.
    fn id_of(named: &str) -> &str {
        named.rsplit('/').next().unwrap_or_default()
    }

    #[test]
    fn a_session_keeps_hearing_after_the_line_that_named_it() {
        // The bug this is here for, and it is the whole feature: `start` read the first line
        // through a reader it then dropped, which closed the pipe. The name arrived, the session
        // looked healthy, and no message ever reached the transcript again -- one line heard,
        // then silence, with nothing anywhere saying so.
        let project = format!("magi-hears-{}", std::process::id());
        let Some((mut layer, _at)) = Melchior::start("melchior", &project, None) else {
            eprintln!("melchior is not installed; skipping");
            return;
        };
        let me = layer.named.clone();
        assert!(!me.is_empty(), "the layer must name the session");

        let mut heard = layer
            .hearing()
            .expect("the pipe is gone after start: nothing could ever arrive");

        let Some((mut them, theirs)) = a_sibling(&project) else {
            eprintln!("melchior is not installed; skipping");
            return;
        };
        let mut sending = Command::new("melchior");
        sending.args([
            "tool",
            "--verb",
            "send",
            "--who",
            id_of(&me),
            "--message",
            "second line",
        ]);
        as_session(&mut sending, &theirs);
        let sent = sending.output().expect("melchior tool runs");
        assert!(
            sent.status.success(),
            "the send failed: {}",
            String::from_utf8_lossy(&sent.stderr)
        );

        // Past the roster, which melchior publishes whenever the set of sessions changes.
        let mut line = String::new();
        while heard.read_line(&mut line).is_ok_and(|read| read > 0) {
            if line.contains("\"message\"") {
                break;
            }
            line.clear();
        }
        let _ = them.kill();
        assert!(
            line.contains("second line") && line.contains(&theirs),
            "the session heard: {line:?}"
        );
        // And it is the shape the driver turns into an entry, not merely text that mentions it.
        let said: Heard = serde_json::from_str(line.trim()).expect("a line magi can read");
        let Heard::Message { who, text, .. } = said else {
            panic!("not a message: {line}");
        };
        assert_eq!(who, theirs);
        assert_eq!(text, "second line");
    }

    #[test]
    fn a_session_hears_every_message_rather_than_the_first() {
        // A pipe read once is not a pipe read: the failure that started this looked exactly like
        // a working session until the second thing arrived.
        let project = format!("magi-again-{}", std::process::id());
        let Some((mut layer, _at)) = Melchior::start("melchior", &project, None) else {
            return;
        };
        let me = layer.named.clone();
        let mut heard = layer.hearing().expect("the pipe");
        let Some((mut them, theirs)) = a_sibling(&project) else {
            return;
        };

        for what in ["one", "two", "three"] {
            let mut sending = Command::new("melchior");
            sending.args([
                "tool",
                "--verb",
                "send",
                "--who",
                id_of(&me),
                "--message",
                what,
            ]);
            as_session(&mut sending, &theirs);
            assert!(sending.output().expect("runs").status.success());
        }

        let mut seen = Vec::new();
        let mut line = String::new();
        while seen.len() < 3 && heard.read_line(&mut line).is_ok_and(|read| read > 0) {
            if let Ok(Heard::Message { text, .. }) = serde_json::from_str::<Heard>(line.trim()) {
                seen.push(text);
            }
            line.clear();
        }
        let _ = them.kill();
        assert_eq!(seen, ["one", "two", "three"], "it stopped listening");
    }

    #[test]
    fn what_the_session_is_doing_keeps_reaching_the_layer() {
        // The other direction, and the same failure mode: a channel that looks fine because the
        // first write succeeded. A sibling asking `status` is told whatever was last said, so
        // one that died after a message reads as a session frozen mid-turn forever.
        let project = format!("magi-doing-{}", std::process::id());
        let Some((mut layer, _at)) = Melchior::start("melchior", &project, None) else {
            return;
        };
        let me = layer.named.clone();
        layer.doing(false, 0, 0);
        layer.doing(true, 41, 2);

        let Some((mut them, theirs)) = a_sibling(&project) else {
            return;
        };
        let mut asking = Command::new("melchior");
        asking.args(["tool", "--verb", "status", "--who", id_of(&me)]);
        as_session(&mut asking, &theirs);
        let asked = asking.output().expect("melchior tool runs");
        let _ = them.kill();
        let said = String::from_utf8_lossy(&asked.stdout).into_owned();
        assert!(
            asked.status.success(),
            "{}",
            String::from_utf8_lossy(&asked.stderr)
        );
        assert!(
            said.contains("41") || said.contains("working"),
            "the layer answered with the state at startup: {said}"
        );
    }
}
