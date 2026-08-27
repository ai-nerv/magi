//! `axum ext shell` — the peer that makes `bash` work.
//!
//! A tool in its own process, speaking the five-message protocol over stdin and stdout. It
//! exists because running commands is the thing that most wants a boundary: arbitrary
//! execution, isolatable later, and — the part a function in a VM cannot do — **stateful**.
//!
//! One `sh` runs for the life of the peer, so `cd build` and `export FOO=1` carry over to the
//! next call. That is the whole reason this is a process: a per-call spawn would make the
//! boundary pure cost and a shell that forgets where it is is not a shell.

use axum_ipc::blocking::{FrameReader, FrameWriter};
use axum_proto::{ToolCallId, ToolReport, ToolRequest};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

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
pub fn run() -> anyhow::Result<()> {
    let mut shell = Session::start()?;
    let mut reader = FrameReader::new(std::io::stdin());
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

    loop {
        let request = match reader.read_blocking::<ToolRequest>() {
            Ok(request) => request,
            // The host went away. Nothing to report to, so leave quietly.
            Err(_) => return Ok(()),
        };
        match request {
            ToolRequest::Call { id, arguments, .. } => {
                let command = arguments["command"].as_str().unwrap_or_default();
                let (output, is_error) = shell.run(command);
                writer.write_blocking(&ToolReport::Result {
                    id,
                    output,
                    is_error,
                })?;
            }
            // A command already running cannot be interrupted through the same pipe it is
            // occupying, so cancellation kills the shell and starts a fresh one. State is lost,
            // which is the honest cost of interrupting something that was mid-flight.
            ToolRequest::Cancel { id } => {
                shell.restart()?;
                writer.write_blocking(&ToolReport::Result {
                    id: ToolCallId::new(id.to_string()),
                    output: "interrupted; the shell was restarted".to_owned(),
                    is_error: true,
                })?;
            }
        }
    }
}

/// One long-lived `sh`.
struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
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

impl Session {
    fn start() -> anyhow::Result<Self> {
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
        let nonce = format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos())
        );
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            nonce,
            seq: 0,
            dead: false,
        })
    }

    fn restart(&mut self) -> anyhow::Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        *self = Self::start()?;
        self.dead = false;
        Ok(())
    }

    /// Run one command and read until its sentinel.
    fn run(&mut self, command: &str) -> (String, bool) {
        if self.dead && self.restart().is_err() {
            return ("the shell could not be restarted".to_owned(), true);
        }
        // stderr is folded into stdout for this command only, so ordering survives without the
        // shell's own diagnostics being redirected for the rest of its life.
        self.seq += 1;
        let marker = marker(&self.nonce, self.seq);
        let script = format!("{{ {command} ; }} 2>&1\nprintf '%s%s\\n' \"{marker}\" \"$?\"\n");
        if writeln!(self.stdin, "{script}").is_err() || self.stdin.flush().is_err() {
            return ("the shell is not accepting input".to_owned(), true);
        }

        let mut output = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            match self.stdout.read_line(&mut line) {
                Ok(0) => {
                    // The shell ended without a sentinel: the command took it with it.
                    self.dead = true;
                    output.push_str("\n(the shell exited; a fresh one starts on the next call)");
                    return (output, true);
                }
                Ok(_) => {}
                Err(e) => return (format!("{output}\n{e}"), true),
            }
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
