//! A tool that lives in its own process.
//!
//! The boundary `bash` justifies: arbitrary execution, a working directory that has to survive
//! between calls, and the thing you would most want to isolate. Any language, crash-isolated,
//! and later sandboxable — none of which a function in a VM can be.
//!
//! **Long-lived and lazily started.** A per-call spawn would make the boundary pure cost:
//! `cd build` then `make` has to work, which means one process holding its own cwd and
//! environment across calls. Started on first use so a declared tool nobody calls costs
//! nothing, and restarted on the next call if it dies.
//!
//! **Reads on its own thread.** A blocking read cannot be given a deadline, so a caller that
//! reads inline can neither time out nor notice an interrupt: it is inside `read` until the
//! peer chooses to answer, and a peer that never answers holds the turn open forever. The
//! thread turns the pipe into a channel, and a channel can be waited on for a bounded time.

use crate::{Cancel, Ops, Output, Tool};
use axon_ipc::blocking::{FrameReader, FrameWriter};
use axon_proto::{ToolCallId, ToolReport, ToolRequest};
use std::cell::RefCell;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long to wait for a peer to answer one call.
///
/// A shell command may legitimately take minutes, so this is generous; it exists to stop a
/// wedged peer holding a turn open forever, not to bound useful work.
const CALL_TIMEOUT: Duration = Duration::from_secs(600);

/// How long this call may take.
///
/// A `timeout` in the arguments, clamped to [`CALL_TIMEOUT`] as a ceiling. Enforced by the host
/// rather than left to the peer: the peer is the thing that might be wedged, and a deadline it
/// enforces itself is one it can fail to. The argument is passed on regardless, so a peer that
/// wants to stop early can.
///
/// The ceiling is not negotiable. A tool that could ask for an hour could hold a turn open for
/// an hour, which is what this constant exists to prevent.
fn allowed(arguments: &serde_json::Value) -> Duration {
    arguments
        .get("timeout")
        .and_then(serde_json::Value::as_u64)
        .map_or(CALL_TIMEOUT, |seconds| {
            Duration::from_secs(seconds.max(1)).min(CALL_TIMEOUT)
        })
}

/// How long a peer has to acknowledge a cancellation before it is killed.
///
/// Short, because the peer is being asked to stop and the user is waiting. A peer that answers
/// keeps its state; one that does not is not in a position to be trusted with it.
const CANCEL_GRACE: Duration = Duration::from_secs(5);

/// How often a waiting call looks up to see whether it is still wanted.
///
/// The interrupt is a flag, not a channel, so it has to be polled. Short enough that `esc`
/// feels immediate, long enough that a running tool costs nothing to wait on.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// How long a peer has to say what it offers.
///
/// Bounded so a peer that declares nothing costs a moment rather than the session. What it
/// costs is the config's claim standing unchallenged, which is where things were before.
const DECLARE_TIMEOUT: Duration = Duration::from_secs(2);

/// How much of a peer's stderr is kept to explain its death.
const COMPLAINT_LIMIT: u64 = 4096;

/// How long to wait for a dead peer's stderr to be collected before giving up on it.
const COMPLAINT_WAIT: Duration = Duration::from_millis(500);

/// A tool reached by talking to a process.
pub struct ProcessTool {
    name: String,
    /// What the config claimed, used until the peer has been asked.
    claimed: Declared,
    /// What the peer said when it was asked, which settles it.
    ///
    /// The peer is the authority. A config can only describe what somebody believed a program
    /// did when they wrote the line, and a model handed a schema the peer does not implement
    /// calls it with the wrong arguments and is told nothing useful about why. Set once,
    /// before any turn: a schema cannot be corrected halfway through a conversation that has
    /// already used it.
    confirmed: std::cell::OnceCell<Declared>,
    command: String,
    args: Vec<String>,
    /// Environment the peer is started with, beside what it inherits.
    env: std::collections::BTreeMap<String, String>,
    /// The running peer, started on first use.
    peer: RefCell<Option<Peer>>,
    /// Calls answered so far, so an id is never reused.
    next: std::cell::Cell<u64>,
    /// The call that has been sent and not yet collected, with how long it may take.
    ///
    /// At most one: a peer answers one call at a time, so a second call to the same tool has
    /// nothing to overlap with the first.
    flight: RefCell<Option<(ToolCallId, Duration)>>,
}

/// A name, a description and a schema, from whichever source is currently believed.
struct Declared {
    description: String,
    parameters: serde_json::Value,
}

struct Peer {
    child: Child,
    /// Reports as they arrive, or the error that ended the stream.
    reports: Receiver<Result<ToolReport, String>>,
    /// The peer's stdin. `Option` so dropping it can close the pipe, which is how a peer is
    /// asked to stop: it reads until its input ends, then runs its own cleanup.
    writer: Option<FrameWriter<std::process::ChildStdin>>,
    /// Whatever the peer complained about on the way down.
    ///
    /// Kept because a peer that fails to start fails on the wire as "broken pipe", which says
    /// nothing anyone can act on. The reason is almost always on its stderr -- a missing
    /// binary, a bad argument, a config error -- and that is the sentence the model and the
    /// user actually need.
    complaint: Arc<Mutex<String>>,
}

/// How long to let a peer finish after its input closes, and how often to look.
///
/// Short: this runs when a registry is torn down, and a peer that has not gone by then is one
/// that is not going to. Long enough that the ordinary case — read EOF, clean up, exit — wins.
const GOODBYE_TRIES: usize = 40;
const GOODBYE_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// How a call ended.
enum Ended {
    /// The peer answered.
    Answered(Output),
    /// The peer is not usable and should be replaced.
    Lost(String),
}

impl ProcessTool {
    /// Declare a tool reached by running `command`.
    #[must_use]
    pub fn new(
        name: &str,
        description: &str,
        parameters: serde_json::Value,
        command: &str,
        args: Vec<String>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            claimed: Declared {
                description: description.to_owned(),
                parameters,
            },
            confirmed: std::cell::OnceCell::new(),
            command: command.to_owned(),
            args,
            env: std::collections::BTreeMap::new(),
            peer: RefCell::new(None),
            next: std::cell::Cell::new(1),
            flight: RefCell::new(None),
        }
    }

    /// Start the peer with these extra environment pairs.
    ///
    /// Builder rather than a sixth argument: it is the one thing about a peer that is usually
    /// nothing, and nine call sites passing an empty map say nothing at any of them.
    #[must_use]
    pub fn with_env(mut self, env: std::collections::BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Start the peer if it is not running.
    fn ensure(&self, ops: &dyn Ops) -> Result<(), String> {
        if self.peer.borrow().is_some() {
            return Ok(());
        }
        let mut command = Command::new(&self.command);
        crate::environ::apply(&mut command, &self.env);
        let mut child = command
            .args(&self.args)
            // Rooted where the session is, so a peer that resolves relative paths agrees with
            // the tools that do not go through it.
            .current_dir(ops.cwd())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Captured rather than inherited. A daemon is started with its own output going
            // nowhere, so an inherited stderr is a diagnostic written to no one; and when a
            // peer dies on startup its complaint is the only thing that explains the failure.
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", self.command))?;

        let stdin = child.stdin.take().ok_or("the peer has no stdin")?;
        let stdout = child.stdout.take().ok_or("the peer has no stdout")?;

        let complaint = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let held = Arc::clone(&complaint);
            std::thread::spawn(move || {
                use std::io::Read;
                let mut said = String::new();
                // Bounded: a chatty peer must not be able to grow this without limit.
                let _ = stderr.take(COMPLAINT_LIMIT).read_to_string(&mut said);
                if let Ok(mut slot) = held.lock() {
                    *slot = said;
                }
            });
        }

        let (reports, incoming) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = FrameReader::new(stdout);
            loop {
                let message = reader
                    .read_blocking::<ToolReport>()
                    .map_err(|e| e.to_string());
                let failed = message.is_err();
                // A closed receiver means the tool was dropped; there is nobody to tell.
                if reports.send(message).is_err() || failed {
                    return;
                }
            }
        });

        *self.peer.borrow_mut() = Some(Peer {
            child,
            reports: incoming,
            writer: Some(FrameWriter::new(stdin)),
            complaint,
        });
        Ok(())
    }

    /// Whatever the peer wrote to its stderr, trimmed.
    /// Waited for, briefly. The draining thread is still finishing when the wire notices the
    /// peer has gone, and reading the slot at once gets an empty string — which is the very
    /// failure this exists to fix. The peer is already dead, so its stderr is closed and the
    /// wait ends as soon as the thread does.
    fn complaint(&self) -> String {
        let deadline = Instant::now() + COMPLAINT_WAIT;
        loop {
            let said = self
                .peer
                .borrow()
                .as_ref()
                .and_then(|peer| peer.complaint.lock().ok().map(|c| c.trim().to_owned()))
                .unwrap_or_default();
            if !said.is_empty() || Instant::now() >= deadline {
                return said;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stop the peer, so the next call starts a fresh one.
    fn drop_peer(&self) {
        if let Some(mut peer) = self.peer.borrow_mut().take() {
            let _ = peer.child.kill();
            let _ = peer.child.wait();
        }
    }

    /// What this tool currently believes it offers.
    fn believed(&self) -> &Declared {
        self.confirmed.get().unwrap_or(&self.claimed)
    }

    /// Read what the peer declares on connect, and take its word for it.
    ///
    /// Bounded, because a peer that never declares is one whose config claim is all there is
    /// -- which is no worse than before it was asked, and better than refusing to run it.
    fn adopt(&self) {
        let mut held = self.peer.borrow_mut();
        let Some(peer) = held.as_mut() else { return };
        let deadline = Instant::now() + DECLARE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            match peer.reports.recv_timeout(remaining) {
                Ok(Ok(ToolReport::Declare {
                    name,
                    description,
                    parameters,
                })) => {
                    // A peer may serve several tools and declares each; this one takes only
                    // its own, and the rest are somebody else's to adopt.
                    if name == self.name {
                        let _ = self.confirmed.set(Declared {
                            description,
                            parameters,
                        });
                        return;
                    }
                }
                // Anything else means the peer has moved on from declaring.
                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => return,
            }
        }
    }

    /// Send one call and wait for the peer to answer it.
    ///
    /// Bounded three ways, because a peer is another program and none of them can be assumed:
    /// the call has a deadline, an interrupt is passed on and then enforced, and a peer whose
    /// stream ends is reported rather than waited on.
    /// What a lost peer is reported as.
    ///
    /// The peer's own words first: "broken pipe" is what the wire saw, and the reason is on its
    /// stderr. A peer that could not start says so there, and without it the failure is
    /// unactionable for both the model and the user.
    fn lost(&self, why: &str) -> Output {
        let said = self.complaint();
        self.drop_peer();
        if said.is_empty() {
            Output::error(format!("{}: {why}", self.name))
        } else {
            Output::error(format!("{}: {why}\n{said}", self.name))
        }
    }

    /// Write the call and return, leaving the answer for [`Tool::wait`].
    ///
    /// Split from the waiting so a round of calls to *different* peers overlaps: the requests go
    /// out one after another and the answers are collected in the order the model asked, so the
    /// round costs the slowest peer rather than the sum of them. See [`crate::Sending`].
    fn post(&self, arguments: &serde_json::Value) -> Result<ToolCallId, String> {
        let id = ToolCallId::new(format!("c{}", self.next.get()));
        self.next.set(self.next.get() + 1);

        let mut held = self.peer.borrow_mut();
        let Some(peer) = held.as_mut() else {
            return Err("the peer is not running".to_owned());
        };
        peer.writer
            .as_mut()
            .ok_or("the peer is closing")?
            .write_blocking(&ToolRequest::Call {
                id: id.clone(),
                name: self.name.clone(),
                arguments: arguments.clone(),
            })
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    fn exchange(&self, id: &ToolCallId, deadline_in: Duration, cancel: &dyn Cancel) -> Ended {
        let mut held = self.peer.borrow_mut();
        let Some(peer) = held.as_mut() else {
            return Ended::Lost("the peer is not running".to_owned());
        };

        let mut deadline = Instant::now() + deadline_in;
        let mut progress = String::new();
        let mut asked_to_stop = false;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ended::Lost(if asked_to_stop {
                    "did not acknowledge the interrupt".to_owned()
                } else {
                    "did not answer within the call timeout".to_owned()
                });
            }

            match peer.reports.recv_timeout(remaining.min(CANCEL_POLL)) {
                Ok(Ok(ToolReport::Progress { id: got, chunk })) if got == *id => {
                    progress.push_str(&chunk);
                }
                Ok(Ok(ToolReport::Result {
                    id: got,
                    output,
                    is_error,
                })) if got == *id => {
                    let mut content = progress;
                    content.push_str(&output);
                    return Ended::Answered(Output { content, is_error });
                }
                // A report for a call that is not this one, or a declaration arriving late.
                // Skipped rather than treated as an answer: the peer may serve several tools.
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Ended::Lost(e),
                Err(RecvTimeoutError::Disconnected) => {
                    return Ended::Lost("stopped answering".to_owned());
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Sent once, then enforced. Asking twice would tell a peer that is already
                    // winding down to start again, and asking forever would never end.
                    if !asked_to_stop && cancel.is_cancelled() {
                        asked_to_stop = true;
                        deadline = Instant::now() + CANCEL_GRACE;
                        if peer.writer.as_mut().is_none_or(|writer| {
                            writer
                                .write_blocking(&ToolRequest::Cancel { id: id.clone() })
                                .is_err()
                        }) {
                            return Ended::Lost("could not be told to stop".to_owned());
                        }
                    }
                }
            }
        }
    }
}

impl Tool for ProcessTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.believed().description
    }

    fn parameters(&self) -> serde_json::Value {
        self.believed().parameters.clone()
    }

    fn probe(&self, ops: &dyn Ops) {
        if self.ensure(ops).is_err() {
            return;
        }
        self.adopt();
    }

    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops, cancel: &dyn Cancel) -> Output {
        // A peer is another program, so the question is asked here rather than trusted to it.
        // A shell command is asked as a *command*, with its program named separately, because
        // "any `git` command" is the answer people actually want to give and they cannot give
        // it if the question was "may the shell tool run".
        if let Some(action) = self.action(arguments)
            && let Err(why) = ops.allow(&self.name, &action)
        {
            return Output::error(why);
        }
        if let Err(why) = self.ensure(ops) {
            return Output::error(why);
        }
        let id = match self.post(arguments) {
            Ok(id) => id,
            Err(why) => return self.lost(&why),
        };

        match self.exchange(&id, allowed(arguments), cancel) {
            Ended::Answered(output) => output,
            Ended::Lost(why) => self.lost(&why),
        }
    }

    fn send(&self, arguments: &serde_json::Value, ops: &dyn Ops) -> crate::Sending {
        // The permission question is asked *here*, in the phase that runs one call at a time,
        // because it is a question to a person: two prompts racing onto one screen is not a
        // faster round, it is an unanswerable one.
        if let Some(action) = self.action(arguments)
            && let Err(why) = ops.allow(&self.name, &action)
        {
            return crate::Sending::Refused(Output::error(why));
        }
        if let Err(why) = self.ensure(ops) {
            return crate::Sending::Refused(Output::error(why));
        }
        // One peer answers one call at a time, so a second call to the *same* tool has nothing
        // to overlap with the first and waits its turn. Overlap is between peers.
        if self.flight.borrow().is_some() {
            return crate::Sending::Inline;
        }
        match self.post(arguments) {
            Ok(id) => {
                *self.flight.borrow_mut() = Some((id, allowed(arguments)));
                crate::Sending::Sent
            }
            Err(why) => crate::Sending::Refused(self.lost(&why)),
        }
    }

    fn wait(&self, cancel: &dyn Cancel) -> Output {
        let Some((id, allowed)) = self.flight.borrow_mut().take() else {
            return Output::error(format!("{}: nothing was sent", self.name));
        };
        match self.exchange(&id, allowed, cancel) {
            Ended::Answered(output) => output,
            Ended::Lost(why) => self.lost(&why),
        }
    }
}

impl Drop for ProcessTool {
    fn drop(&mut self) {
        self.drop_peer();
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[test]
    fn no_timeout_asked_for_is_the_ceiling() {
        assert_eq!(allowed(&serde_json::json!({})), CALL_TIMEOUT);
    }

    #[test]
    fn a_short_timeout_is_honoured() {
        // The point: `bash` with something that may hang should not hold the turn for ten
        // minutes before anyone finds out.
        assert_eq!(
            allowed(&serde_json::json!({ "timeout": 5 })),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn the_ceiling_is_not_negotiable() {
        // A tool that could ask for an hour could hold a turn open for an hour.
        assert_eq!(
            allowed(&serde_json::json!({ "timeout": 86_400 })),
            CALL_TIMEOUT
        );
    }

    #[test]
    fn zero_is_not_an_instant_failure() {
        // A model that sends 0 means "be quick", not "give up before starting".
        assert_eq!(
            allowed(&serde_json::json!({ "timeout": 0 })),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn a_timeout_that_is_not_a_number_is_ignored() {
        assert_eq!(
            allowed(&serde_json::json!({ "timeout": "soon" })),
            CALL_TIMEOUT
        );
    }
}

impl ProcessTool {
    /// What this call is about to do, in the terms a person is asked about.
    ///
    /// Only a `command` argument is recognised, which is the shell's. A peer that takes
    /// something else is not asked about — its own schema is the description of what it does,
    /// and inventing an action from arguments this code does not understand would put a
    /// sentence in front of somebody that does not mean what it says.
    fn action(&self, arguments: &serde_json::Value) -> Option<axon_proto::permit::Action> {
        let command = arguments.get("command")?.as_str()?;
        Some(axon_proto::permit::Action::Run {
            command: command.to_owned(),
            program: first_word(command),
        })
    }
}

/// The program a command line runs, for the "any `git` command" answer.
///
/// Leading environment assignments are stepped over: `FOO=1 git status` is a `git` command, and
/// a person offered "any `FOO=1` command" would rightly not know what they were being asked.
fn first_word(command: &str) -> String {
    command
        .split_whitespace()
        .find(|word| !word.contains('=') || word.starts_with('/'))
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod action_tests {
    use super::*;

    #[test]
    fn a_shell_call_is_asked_about_as_a_command() {
        assert_eq!(first_word("git status --short"), "git");
    }

    #[test]
    fn leading_environment_is_stepped_over() {
        // Somebody offered "any `FOO=1` command" would rightly not know what was being asked.
        assert_eq!(first_word("FOO=1 BAR=2 git push"), "git");
    }

    #[test]
    fn an_absolute_path_is_the_program_even_with_an_equals_in_it() {
        assert_eq!(first_word("/usr/bin/env python"), "/usr/bin/env");
    }

    #[test]
    fn an_empty_command_names_no_program() {
        assert_eq!(first_word("   "), "");
    }
}

impl Drop for Peer {
    /// Stop the peer, so it can stop whatever it started.
    ///
    /// A `Child` that is merely dropped is leaked: Rust neither kills nor reaps it. The peer
    /// then outlives the registry that owned it, and so does anything it spawned — the shell
    /// peer's own shell among them, which is how a test run left two and a half thousand
    /// orphaned `bash` processes parented to init.
    ///
    /// Killed rather than asked, because the ask is "close its stdin" and stdin is owned by the
    /// writer this cannot move out of. The shell peer's shell sits on a pty whose controller
    /// dies with the peer, which is what takes it down in turn.
    fn drop(&mut self) {
        // Closing its input first, because that is how a peer is told to stop, and stopping is
        // what lets its own cleanup run — the shell peer kills the shell it started there. A
        // `Child` that is merely dropped is leaked: Rust neither kills nor reaps it.
        self.writer = None;
        for _ in 0..GOODBYE_TRIES {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(GOODBYE_POLL),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
