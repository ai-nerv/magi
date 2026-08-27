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
use axum_ipc::blocking::{FrameReader, FrameWriter};
use axum_proto::{ToolCallId, ToolReport, ToolRequest};
use std::cell::RefCell;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long to wait for a peer to answer one call.
///
/// A shell command may legitimately take minutes, so this is generous; it exists to stop a
/// wedged peer holding a turn open forever, not to bound useful work.
const CALL_TIMEOUT: Duration = Duration::from_secs(600);

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
    /// The running peer, started on first use.
    peer: RefCell<Option<Peer>>,
    /// Calls answered so far, so an id is never reused.
    next: std::cell::Cell<u64>,
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
    writer: FrameWriter<std::process::ChildStdin>,
}

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
            peer: RefCell::new(None),
            next: std::cell::Cell::new(1),
        }
    }

    /// Start the peer if it is not running.
    fn ensure(&self, ops: &dyn Ops) -> Result<(), String> {
        if self.peer.borrow().is_some() {
            return Ok(());
        }
        let mut child = Command::new(&self.command)
            .args(&self.args)
            // Rooted where the session is, so a peer that resolves relative paths agrees with
            // the tools that do not go through it.
            .current_dir(ops.cwd())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Left alone: a peer's diagnostics belong on the daemon's stderr, not swallowed
            // into a tool result where they would read as model-facing output.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", self.command))?;

        let stdin = child.stdin.take().ok_or("the peer has no stdin")?;
        let stdout = child.stdout.take().ok_or("the peer has no stdout")?;

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
            writer: FrameWriter::new(stdin),
        });
        Ok(())
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
    fn exchange(
        &self,
        id: &ToolCallId,
        arguments: &serde_json::Value,
        cancel: &dyn Cancel,
    ) -> Ended {
        let mut held = self.peer.borrow_mut();
        let Some(peer) = held.as_mut() else {
            return Ended::Lost("the peer is not running".to_owned());
        };

        if let Err(e) = peer.writer.write_blocking(&ToolRequest::Call {
            id: id.clone(),
            name: self.name.clone(),
            arguments: arguments.clone(),
        }) {
            return Ended::Lost(e.to_string());
        }

        let mut deadline = Instant::now() + CALL_TIMEOUT;
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
                        if peer
                            .writer
                            .write_blocking(&ToolRequest::Cancel { id: id.clone() })
                            .is_err()
                        {
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
        if let Err(why) = self.ensure(ops) {
            return Output::error(why);
        }
        let id = ToolCallId::new(format!("c{}", self.next.get()));
        self.next.set(self.next.get() + 1);

        match self.exchange(&id, arguments, cancel) {
            Ended::Answered(output) => output,
            Ended::Lost(why) => {
                // A peer that died, wedged or ignored an interrupt takes its state with it, so
                // the next call starts fresh rather than writing into a pipe nobody reads.
                self.drop_peer();
                Output::error(format!("{}: {why}", self.name))
            }
        }
    }
}

impl Drop for ProcessTool {
    fn drop(&mut self) {
        self.drop_peer();
    }
}
