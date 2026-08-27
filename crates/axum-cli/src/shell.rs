//! `axum ext shell` — the peer that makes `bash` work.
//!
//! A tool in its own process, speaking the five-message protocol over stdin and stdout. It
//! exists because running commands is the thing that most wants a boundary: arbitrary
//! execution, isolatable later, and — the part a function in a VM cannot do — **stateful**.
//!
//! One `sh` runs for the life of the peer, so `cd build` and `export FOO=1` carry over to the
//! next call. That is the whole reason this is a process: a per-call spawn would make the
//! boundary pure cost and a shell that forgets where it is is not a shell.
//!
//! **Three threads, because a peer that can only be interrupted between calls cannot be
//! interrupted at all.** One reads requests from the host, one reads the shell's output, and
//! the main thread runs commands. Nothing here ever blocks on a read it cannot abandon.

use axum_ipc::blocking::{FrameReader, FrameWriter};
use axum_proto::{ToolCallId, ToolReport, ToolRequest};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

/// How often a running command looks up to see whether it is still wanted.
const INTERRUPT_POLL: Duration = Duration::from_millis(25);

/// Written after every command so the reader knows where its output ended.
///
/// A persistent shell gives no other signal: the stream does not close between commands, so
/// without a marker a reader cannot tell "finished" from "still thinking". The exit status
/// rides along because `$?` is only meaningful on the line right after.
///
/// Unguessable per session and per command, for two reasons that pull in opposite directions.
/// Output that does not end in a newline runs into the marker on the same line, so the reader
/// has to find it anywhere rather than only at the start -- and a marker that can be found
/// anywhere is one a command could print to fake an ending. A nonce it cannot know settles
/// both: `cat` a file with no trailing newline works, and nothing can counterfeit the end.
fn marker(nonce: &str, seq: u64) -> String {
    format!("__axum_{nonce}_{seq}__")
}

/// Run the peer until its input closes.
///
/// The request reader is a thread of its own because this one is inside the command an
/// interrupt is asking it to abandon. A peer that reads only between calls leaves
/// `ToolRequest::Cancel` sitting unread in a pipe until the thing it was meant to stop has
/// finished on its own, which is the same as not implementing it.
pub fn run() -> anyhow::Result<()> {
    let mut shell = Session::start()?;
    let mut writer = FrameWriter::new(std::io::stdout());

    // Declared on connect rather than configured by the host: the peer is the only thing that
    // knows what it can actually do.
    writer.write_blocking(&ToolReport::Declare {
        name: "bash".to_owned(),
        description: "Run a shell command. The working directory and environment persist \
                      between calls."
            .to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command line to run." },
            },
            "required": ["command"],
        }),
    })?;

    let (calls, incoming) = std::sync::mpsc::channel::<(ToolCallId, String)>();
    let interrupted = Arc::clone(&shell.interrupted);
    std::thread::spawn(move || {
        let mut reader = FrameReader::new(std::io::stdin());
        loop {
            match reader.read_blocking::<ToolRequest>() {
                Ok(ToolRequest::Call { id, arguments, .. }) => {
                    let command = arguments["command"].as_str().unwrap_or_default().to_owned();
                    if calls.send((id, command)).is_err() {
                        return;
                    }
                }
                // Raising a flag is the whole of it. Acting on it belongs to the thread that
                // is waiting on the command, because that is the thread that knows what it is
                // waiting for and the only one that can stop.
                Ok(ToolRequest::Cancel { .. }) => interrupted.store(true, Ordering::SeqCst),
                // The host went away. Nothing to report to, so leave quietly.
                Err(_) => return,
            }
        }
    });

    while let Ok((id, command)) = incoming.recv() {
        let (output, is_error) = shell.run(&command);
        writer.write_blocking(&ToolReport::Result {
            id,
            output,
            is_error,
        })?;
    }
    Ok(())
}

/// One long-lived `sh`.
struct Session {
    child: Child,
    stdin: ChildStdin,
    /// The shell's output, a line at a time.
    ///
    /// A channel rather than the pipe itself, because a pipe cannot be read with a deadline
    /// and an interrupt that has to wait for the next line is not an interrupt. Killing the
    /// shell does not help: a command that spawned anything leaves that child holding the
    /// same pipe open, so the read blocks on for as long as the thing being interrupted runs.
    lines: Receiver<String>,
    /// Raised by the request reader when the host asks for a stop.
    interrupted: Arc<AtomicBool>,
    /// Unguessable per-session half of the end-of-command marker.
    nonce: String,
    /// Commands run so far, which is the other half.
    seq: u64,
    /// Whether the shell has gone, so the next call starts a fresh one.
    ///
    /// A command may legitimately end its shell -- `exit`, or something that kills it -- and
    /// running it in a subshell instead would cost the persistence that is the whole reason
    /// this is a process. So the death is expected and recovered from rather than prevented.
    dead: bool,
}

/// A per-shell value a command cannot guess.
fn nonce() -> String {
    format!(
        "{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
    )
}

/// Spawn one shell, and a thread turning its output into lines.
fn spawn_shell() -> anyhow::Result<(Child, ChildStdin, Receiver<String>)> {
    let mut child = Command::new("sh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Merged into stdout rather than read separately: two pipes cannot be interleaved
        // faithfully, and a build's errors belong where they happened in its output.
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdout"))?;

    let (lines, incoming) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                // A closed receiver means this shell was abandoned. The thread outlives it
                // only until whatever still holds the pipe lets go, which is exactly as long
                // as the interrupted command keeps running.
                Ok(_) => {
                    if lines.send(std::mem::take(&mut line)).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Ok((child, stdin, incoming))
}

impl Session {
    fn start() -> anyhow::Result<Self> {
        let (child, stdin, lines) = spawn_shell()?;
        Ok(Self {
            child,
            stdin,
            lines,
            interrupted: Arc::new(AtomicBool::new(false)),
            nonce: nonce(),
            seq: 0,
            dead: false,
        })
    }

    /// Replace the shell, keeping the interrupt flag the reader thread already holds.
    fn restart(&mut self) -> anyhow::Result<()> {
        let (child, stdin, lines) = spawn_shell()?;
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.child = child;
        self.stdin = stdin;
        self.lines = lines;
        // A fresh marker, because a line from the abandoned shell arriving late must not be
        // able to end a command in this one.
        self.nonce = nonce();
        self.seq = 0;
        self.dead = false;
        Ok(())
    }

    /// Run one command and read until its marker, or until the host calls it off.
    fn run(&mut self, command: &str) -> (String, bool) {
        if self.dead && self.restart().is_err() {
            return ("the shell could not be restarted".to_owned(), true);
        }
        // A stop raised while nothing was running belongs to nothing, and left set it would
        // cancel the next command instead.
        self.interrupted.store(false, Ordering::SeqCst);

        // stderr is folded into stdout for this command only, so ordering survives without the
        // shell's own diagnostics being redirected for the rest of its life.
        self.seq += 1;
        let marker = marker(&self.nonce, self.seq);
        let script = format!("{{ {command} ; }} 2>&1\nprintf '%s%s\\n' \"{marker}\" \"$?\"\n");
        if writeln!(self.stdin, "{script}").is_err() || self.stdin.flush().is_err() {
            return ("the shell is not accepting input".to_owned(), true);
        }

        let mut output = String::new();
        loop {
            if self.interrupted.swap(false, Ordering::SeqCst) {
                // Abandoned rather than waited out. The shell is killed and a fresh one starts
                // on the next call; anything the command had spawned may outlive it, which is
                // the honest cost of interrupting something mid-flight.
                self.dead = true;
                output.push_str("\n(interrupted; a fresh shell starts on the next call)");
                return (output, true);
            }
            match self.lines.recv_timeout(INTERRUPT_POLL) {
                Ok(line) => {
                    if let Some(at) = line.find(&marker) {
                        // Anything before the marker is output that did not end in a newline.
                        output.push_str(&line[..at]);
                        let code = line[at + marker.len()..].trim_end();
                        let failed = code != "0";
                        if failed {
                            output.push_str(&format!("\n(exit {code})"));
                        }
                        return (output, failed);
                    }
                    output.push_str(&line);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    // The shell ended without a marker: the command took it with it.
                    self.dead = true;
                    output.push_str("\n(the shell exited; a fresh one starts on the next call)");
                    return (output, true);
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_returns_its_output() {
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("echo hello");
        assert_eq!(output.trim(), "hello");
        assert!(!failed);
    }

    #[test]
    fn state_persists_between_calls() {
        // The reason this is a process rather than a function: a shell that forgets where it
        // is is not a shell.
        let mut shell = Session::start().expect("a shell");
        shell.run("cd /tmp");
        let (output, _) = shell.run("pwd");
        assert_eq!(output.trim(), "/tmp");

        shell.run("export AXUM_TEST_VAR=carried");
        let (output, _) = shell.run("echo $AXUM_TEST_VAR");
        assert_eq!(output.trim(), "carried");
    }

    #[test]
    fn a_failing_command_reports_its_status_and_its_output() {
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("echo before; false");
        assert!(failed);
        assert!(output.contains("before"), "{output}");
        assert!(output.contains("exit 1"), "{output}");
    }

    #[test]
    fn a_command_that_ends_the_shell_is_survived() {
        // `exit` is a legitimate thing to run, and a subshell would cost the persistence that
        // is the reason this is a process at all. So the death is recovered from.
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("echo before; exit 3");
        assert!(failed);
        assert!(output.contains("before"), "{output}");
        assert!(output.contains("fresh one"), "{output}");

        let (output, failed) = shell.run("echo after");
        assert!(!failed, "the next call works: {output}");
        assert_eq!(output.trim(), "after");
    }

    #[test]
    fn stderr_is_interleaved_with_stdout_in_order() {
        let mut shell = Session::start().expect("a shell");
        let (output, _) = shell.run("echo one; echo two >&2; echo three");
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines, ["one", "two", "three"], "ordering must survive");
    }

    #[test]
    fn a_command_producing_nothing_still_answers() {
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("true");
        assert_eq!(output, "");
        assert!(!failed);
    }

    #[test]
    fn output_that_does_not_end_in_a_newline_still_ends_the_read() {
        // `cat` on a file with no trailing newline runs straight into the marker, which is
        // what hung the first version of this.
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("printf no-newline");
        assert_eq!(output, "no-newline");
        assert!(!failed);
    }

    #[test]
    fn a_command_cannot_counterfeit_the_end_of_its_own_output() {
        // The marker is found anywhere on a line, so it has to be one a command cannot guess.
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("echo '__axum_done__0'; echo after");
        assert!(output.contains("after"), "{output}");
        assert!(!failed);
    }

    #[test]
    fn a_restart_gives_a_fresh_shell() {
        let mut shell = Session::start().expect("a shell");
        shell.run("cd /tmp");
        shell.restart().expect("restart");
        let (output, _) = shell.run("pwd");
        assert_ne!(output.trim(), "/tmp", "state is lost, which is the cost");
    }
}
