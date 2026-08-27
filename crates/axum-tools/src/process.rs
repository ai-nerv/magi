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

use crate::{Ops, Output, Tool};
use axum_ipc::blocking::{FrameReader, FrameWriter};
use axum_proto::{ToolCallId, ToolReport, ToolRequest};
use std::cell::RefCell;
use std::process::{Child, Command, Stdio};

/// How long to wait for a peer to answer one call.
///
/// A shell command may legitimately take minutes, so this is generous; it exists to stop a
/// wedged peer holding a turn open forever, not to bound useful work.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// A tool reached by talking to a process.
pub struct ProcessTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    command: String,
    args: Vec<String>,
    /// The running peer, started on first use.
    peer: RefCell<Option<Peer>>,
    /// Calls answered so far, so an id is never reused.
    next: std::cell::Cell<u64>,
}

struct Peer {
    child: Child,
    reader: FrameReader<std::process::ChildStdout>,
    writer: FrameWriter<std::process::ChildStdin>,
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
            description: description.to_owned(),
            parameters,
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
        *self.peer.borrow_mut() = Some(Peer {
            child,
            reader: FrameReader::new(stdout),
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

    /// Send one call and read until it answers.
    fn exchange(&self, id: &ToolCallId, arguments: &serde_json::Value) -> Result<Output, String> {
        let mut held = self.peer.borrow_mut();
        let peer = held.as_mut().ok_or("the peer is not running")?;

        peer.writer
            .write_blocking(&ToolRequest::Call {
                id: id.clone(),
                name: self.name.clone(),
                arguments: arguments.clone(),
            })
            .map_err(|e| e.to_string())?;

        let deadline = std::time::Instant::now() + CALL_TIMEOUT;
        let mut progress = String::new();
        loop {
            if std::time::Instant::now() > deadline {
                return Err(format!(
                    "{} did not answer within the call timeout",
                    self.name
                ));
            }
            match peer.reader.read_blocking::<ToolReport>() {
                Ok(ToolReport::Progress { id: got, chunk }) if got == *id => {
                    progress.push_str(&chunk);
                }
                Ok(ToolReport::Result {
                    id: got,
                    output,
                    is_error,
                }) if got == *id => {
                    let mut content = progress;
                    content.push_str(&output);
                    return Ok(Output { content, is_error });
                }
                // A report for a call that is not this one, or a declaration arriving late.
                // Skipped rather than treated as an answer: the peer may serve several tools.
                Ok(_) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

impl Tool for ProcessTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops) -> Output {
        if let Err(why) = self.ensure(ops) {
            return Output::error(why);
        }
        let id = ToolCallId::new(format!("c{}", self.next.get()));
        self.next.set(self.next.get() + 1);

        match self.exchange(&id, arguments) {
            Ok(output) => output,
            Err(why) => {
                // A peer that died takes its state with it, so the next call starts fresh
                // rather than talking to a socket nobody is reading.
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
