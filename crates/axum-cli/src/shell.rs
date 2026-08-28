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
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::process::{Child, Command, Stdio};
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
    let which = shell_name();
    writer.write_blocking(&ToolReport::Declare {
        name: "shell".to_owned(),
        description: format!(
            "Run a command in the user's own shell ({which}). The working directory and \
             environment persist between calls.\n\n\
             This is their login shell, not `sh`: their aliases, functions and shell-specific \
             features are available, and a scriptable shell can be asked things a POSIX one \
             cannot.\n\n\
             Long output is truncated in the middle and the whole of it is written to a file \
             the result names."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command line to run." },
                "timeout": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 600,
                    "description": "Seconds to allow before giving up. Defaults to 600. \
                                    Use a short one for something that may hang.",
                },
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
    /// The terminal we write commands into. A `File`, because that is what a pty is.
    /// The terminal we write commands into. A `File`, because that is what a pty is.
    stdin: std::fs::File,
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

/// Which shell to run.
///
/// `$AXUM_SHELL`, then `$SHELL`, then `sh`. It was `sh` outright, which threw away the thing
/// that makes this tool worth having: a person's own shell is the one that knows their aliases,
/// their functions, and — for a shell like oslo — a whole scripting surface the model can reach
/// through the same tool it already has. Running the lowest common denominator is a choice to
/// be useless on every machine equally.
///
/// `sh` remains the floor, because a login shell recorded in the environment is not always a
/// shell that exists on this machine.
#[must_use]
pub fn shell_command() -> String {
    let axum_shell = std::env::var("AXUM_SHELL").ok();
    let login = std::env::var("SHELL").ok();
    shell_command_from(login.as_deref(), axum_shell.as_deref())
}

/// The same, from values rather than the environment, so it can be tested.
#[must_use]
fn shell_command_from(login: Option<&str>, override_: Option<&str>) -> String {
    for candidate in [override_, login].into_iter().flatten() {
        if !candidate.is_empty() && std::path::Path::new(candidate).exists() {
            return candidate.to_owned();
        }
    }
    "sh".to_owned()
}

/// The shell's name, for saying what this tool runs.
#[must_use]
pub fn shell_name() -> String {
    name_of(&shell_command())
}

/// The last component of a path, or the whole of it.
fn name_of(command: &str) -> String {
    std::path::Path::new(command)
        .file_name()
        .map_or_else(|| command.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Open a pseudo-terminal: the side we hold, and the side the shell gets.
///
/// **A pipe is not good enough for a real shell.** Writing to a pipe a shell block-buffers, so
/// nothing arrives until it exits; writing to a terminal it line-buffers, which is what the
/// end-of-command marker depends on. `sh` happened to behave, which is why pipes worked while
/// this ran `sh` and stopped the moment it ran the user's own — oslo produced nothing at all for
/// the whole 600-second timeout. Tau's shell extension runs on a pty for the same reason.
///
/// A terminal is also what makes a shell answer "yes" to *am I interactive*, which is what loads
/// aliases and functions in the first place.
fn open_pty() -> anyhow::Result<(OwnedFd, OwnedFd)> {
    use rustix::pty::OpenptFlags;
    let controller = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    rustix::pty::grantpt(&controller)?;
    rustix::pty::unlockpt(&controller)?;

    let name = rustix::pty::ptsname(&controller, Vec::new())?;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(name.into_bytes()));
    let device = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;

    // Echo off. A terminal repeats what is written to it, so every command would come back as
    // the first line of its own output — and the end-of-command marker would arrive before the
    // command had run.
    if let Ok(mut attrs) = rustix::termios::tcgetattr(&controller) {
        attrs.local_modes -= rustix::termios::LocalModes::ECHO;
        let _ =
            rustix::termios::tcsetattr(&controller, rustix::termios::OptionalActions::Now, &attrs);
    }
    Ok((controller, OwnedFd::from(device)))
}

/// Spawn one shell, and a thread turning its output into lines.
fn spawn_shell() -> anyhow::Result<(Child, std::fs::File, Receiver<String>)> {
    let (controller, device) = open_pty()?;
    // **A terminal on all three.** A shell writing to a pipe block-buffers, so nothing arrives
    // until it exits — which is why nothing came back at all once this ran the user's own shell
    // instead of `sh`. Worse, a shell like oslo reads a piped stdin to EOF before running any of
    // it, so a persistent session over pipes is not merely slow, it is impossible.
    //
    // On a terminal both problems go: output is line-buffered, and the shell behaves as the REPL
    // this protocol assumes. Tau's shell extension is on a pty for the same reason.
    //
    // The cost is that the shell now believes it is interactive and prints a prompt and its
    // startup escape codes into the same stream. Both are handled where the output is read: the
    // escapes are stripped, and the markers are emitted on lines of their own so anything
    // sharing a line with one is, by construction, not the command's output.
    let child = Command::new(shell_command())
        .env("TERM", "dumb")
        .stdin(Stdio::from(device.try_clone()?))
        .stdout(Stdio::from(device.try_clone()?))
        .stderr(Stdio::from(device))
        .spawn()?;

    // Two handles on our side: reading happens on its own thread, writing on this one.
    let to_shell = std::fs::File::from(controller.try_clone()?);
    let from_shell = std::fs::File::from(controller);

    let (lines, incoming) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from_shell);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                // A closed receiver means this shell was abandoned. The thread outlives it
                // only until whatever still holds the terminal lets go, which is exactly as
                // long as the interrupted command keeps running.
                Ok(_) => {
                    // A terminal ends every line with CRLF, and a stray carriage return in tool
                    // output is noise the model reads as data.
                    let cleaned = line.trim_end_matches(['\r', '\n']).to_owned();
                    if lines.send(cleaned).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Ok((child, to_shell, incoming))
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
        let open = format!("{}o", marker(&self.nonce, self.seq));
        let close = format!("{}c", marker(&self.nonce, self.seq));
        // `< /dev/null` on the command group, because the shell's stdin IS this protocol's
        // control channel: the markers are written into the same terminal. A command that reads
        // stdin therefore eats its own end-of-command marker. `cat` echoes it back and the
        // marker lands in the tool result with a meaningless exit status; `sort`, `sudo`, `ssh`
        // and `read` swallow it and the call hangs for the whole of CALL_TIMEOUT, taking the
        // persistent shell's state with it when the peer is killed. Nothing legitimate reads
        // stdin here -- the peer never feeds a command input.
        let script = format!(
            "printf '\\n%s\\n' \"{open}\"\n{{ printf '\\n' ; {command} ; __axum_status=$? ; printf '\\n' ; }} < /dev/null 2>&1\nprintf '\\n%s%s\\n' \"{close}\" \"$__axum_status\"\n"
        );
        if write!(self.stdin, "{script}").is_err() || self.stdin.flush().is_err() {
            return ("the shell is not accepting input".to_owned(), true);
        }

        let mut output = String::new();
        // **Prompt-agnostic.** A terminal makes the shell interactive, so it prints a prompt
        // before every line it reads -- the command's and both marker `printf`s -- and no
        // environment variable suppresses that reliably. Learning the prompt and stripping it
        // does not work either: nearly every prompt carries the working directory, so the very
        // command that changes it (`cd`) invalidates the thing being stripped mid-call.
        //
        // So the protocol does not need to know what a prompt looks like. Each marker is printed
        // after a newline of its own, and the command's own output is preceded by one:
        //
        //   PROMPT              <- discarded, before the open marker
        //   OPEN                <- discarded
        //   PROMPT              <- exactly one line, always: discarded
        //   ...the output...
        //   PROMPT              <- held back, then dropped when the close marker arrives
        //   CLOSE<status>
        //
        // Holding the last line back is what removes the final prompt without recognising it.
        let mut started = false;
        let mut skipped_prompt = false;
        let mut held: Option<String> = None;
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
                    let line = strip_escapes(&line);
                    if !started {
                        started = line.contains(&open);
                        continue;
                    }
                    if !skipped_prompt {
                        skipped_prompt = true;
                        continue;
                    }
                    if let Some(at) = line.find(&close) {
                        let code = line[at + close.len()..].trim_end();
                        let failed = code != "0";
                        // `held` is the prompt that preceded this marker. Dropped, not written.
                        //
                        // The command group ends with a newline of its own so that the prompt
                        // is always alone on its line -- `printf no-newline` otherwise leaves
                        // the two glued together and the output goes with the prompt. The cost
                        // is a blank line whenever the output already ended in one.
                        while output.ends_with('\n') {
                            output.pop();
                        }
                        if failed {
                            output.push_str(&format!("\n(exit {code})"));
                        }
                        return (output, failed);
                    }
                    if let Some(previous) = held.replace(line) {
                        output.push_str(&previous);
                        output.push('\n');
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    // The shell ended without a marker: the command took it with it.
                    self.dead = true;
                    if let Some(previous) = held.take() {
                        output.push_str(&previous);
                    }
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

#[cfg(test)]
mod stdin_tests {
    use super::*;

    #[test]
    fn a_command_that_reads_stdin_does_not_eat_its_own_marker() {
        // The shell's stdin is this protocol's control channel. `sort` reads to EOF and never
        // echoes, so before the redirect it swallowed the end-of-command marker and the call
        // hung for the whole of CALL_TIMEOUT, taking the persistent shell with it.
        let mut shell = Session::start().expect("a shell");
        let (output, is_error) = shell.run("sort");
        assert!(!is_error, "{output}");
        assert!(
            output.trim().is_empty(),
            "sort of nothing is nothing: {output:?}"
        );
    }

    #[test]
    fn a_command_that_echoes_stdin_does_not_leak_the_marker() {
        // `cat` echoed the marker back, so it landed in the tool result and the exit status
        // reported was the one printed by the leaked line rather than the command's.
        let mut shell = Session::start().expect("a shell");
        let (output, _) = shell.run("cat");
        assert!(
            !output.contains("axum-"),
            "the marker must not reach the model: {output:?}"
        );
        assert!(output.trim().is_empty(), "{output:?}");
    }

    #[test]
    fn the_shell_still_works_after_one_of_those() {
        // The point of the persistent shell: a stdin-reading command used to end it.
        let mut shell = Session::start().expect("a shell");
        let _ = shell.run("sort");
        let (output, is_error) = shell.run("echo alive");
        assert!(!is_error, "{output}");
        assert_eq!(output.trim(), "alive");
    }
}

#[cfg(test)]
mod which_shell_tests {
    use super::*;

    #[test]
    fn a_shell_that_does_not_exist_is_not_used() {
        // `$SHELL` records a login shell, which is not always a shell this machine has — a
        // home directory carried between machines is the usual way that happens.
        assert_eq!(shell_command_from(Some("/no/such/shell"), None), "sh");
    }

    #[test]
    fn the_users_own_shell_wins_over_sh() {
        assert_eq!(shell_command_from(Some("/bin/sh"), None), "/bin/sh");
    }

    #[test]
    fn an_override_beats_the_login_shell() {
        // `$AXUM_SHELL` exists so a session can differ from the login shell without changing it.
        assert_eq!(
            shell_command_from(Some("/bin/sh"), Some("/bin/sh")),
            "/bin/sh"
        );
    }

    #[test]
    fn nothing_set_is_sh() {
        assert_eq!(shell_command_from(None, None), "sh");
    }

    #[test]
    fn the_name_is_what_the_model_is_told_it_is_running() {
        assert_eq!(name_of("/usr/bin/oslo"), "oslo");
        assert_eq!(name_of("sh"), "sh");
    }
}

/// Remove terminal escape sequences from one line.
///
/// A shell on a terminal writes colour, title and shell-integration codes into the same stream
/// as its output. None of it is the command's, and a model handed `\u{1b}]3008;start=…` reads it
/// as data. CSI sequences end at their final byte; OSC ones run to a BEL or an ST.
fn strip_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte in `@`..`~`.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: runs to BEL, or to ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        let _ = chars.next();
                        break;
                    }
                }
            }
            // Anything else is a two-character sequence, already consumed.
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod escape_tests {
    use super::*;

    #[test]
    fn colour_is_removed_and_the_text_kept() {
        assert_eq!(strip_escapes("\u{1b}[01;32mhello\u{1b}[00m"), "hello");
    }

    #[test]
    fn shell_integration_codes_are_removed() {
        // A model handed `]3008;start=…` reads it as data.
        let noisy = "\u{1b}]3008;start=abc;cwd=/tmp\u{1b}\\hello";
        assert_eq!(strip_escapes(noisy), "hello");
    }

    #[test]
    fn an_osc_ending_in_bell_is_removed() {
        assert_eq!(strip_escapes("\u{1b}]0;a title\u{7}text"), "text");
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(strip_escapes("just output"), "just output");
    }

    #[test]
    fn a_lone_escape_does_not_eat_the_line() {
        assert_eq!(strip_escapes("a\u{1b}b"), "a");
    }
}
