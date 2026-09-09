//! `magi ext shell` — the peer that makes `bash` work: a tool in its own process, speaking the
//! five-message protocol over stdin and stdout. One `sh` runs for the life of the peer, so `cd` and
//! `export` carry over to the next call. Three threads — one reads requests from the host, one
//! reads the shell's output, one runs commands — because a peer that can only be interrupted
//! between calls cannot be interrupted at all.

use magi_ipc::blocking::{FrameReader, FrameWriter};
use magi_proto::{ToolCallId, ToolReport, ToolRequest};
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

/// Written after every command so the reader knows where its output ended: a persistent shell gives
/// no other signal, and the exit status rides along because `$?` is only meaningful on the next
/// line. Unguessable per session and per command, because output with no trailing newline runs into
/// the marker on the same line and a marker found anywhere is one a command could counterfeit.
fn marker(nonce: &str, seq: u64) -> String {
    format!("__magi_{nonce}_{seq}__")
}

/// Run the peer until its input closes. The request reader is a thread of its own because this one
/// is inside the command an interrupt is asking it to abandon.
pub fn run() -> anyhow::Result<()> {
    let mut shell = Session::start()?;
    let mut writer = FrameWriter::new(std::io::stdout());

    // Declared on connect rather than configured by the host: the peer knows what it can do.
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
                // Raising a flag is the whole of it: acting on it belongs to the thread waiting on
                // the command, which is the only one that can stop.
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
    stdin: std::fs::File,
    /// A channel rather than the pipe, because a pipe cannot be read with a deadline. Killing
    /// the shell does not help: a command that spawned anything holds the same pipe open.
    lines: Receiver<String>,
    /// The named pipe the output comes down, kept only to unlink: the reader removes the name as
    /// soon as both ends are open, so this is for the shell that never opened its end.
    fifo: std::path::PathBuf,
    interrupted: Arc<AtomicBool>,
    nonce: String,
    seq: u64,
    /// Whether the shell has gone, so the next call starts a fresh one: `exit` is a legitimate
    /// thing to run, and a subshell would cost the persistence this process exists for.
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

/// Which shell to run: `$MAGI_SHELL`, then `$SHELL`, then `sh`. A person's own shell knows their
/// aliases and functions; `sh` is the floor, because a recorded login shell may not exist here.
#[must_use]
pub fn shell_command() -> String {
    let magi_shell = std::env::var("MAGI_SHELL").ok();
    let login = std::env::var("SHELL").ok();
    shell_command_from(login.as_deref(), magi_shell.as_deref())
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

/// Open a pseudo-terminal: the side we hold, and the side the shell gets. A shell writing to a pipe
/// block-buffers, so nothing arrives until it exits; on a terminal it line-buffers, which is what
/// the end-of-command marker depends on. A terminal is also what makes a shell answer yes to *am I
/// interactive*, which is what loads aliases — and why `TERM` is `dumb`, so it draws no prompt.
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

    // Echo off: a terminal repeats what is written to it, so every command would come back as the
    // first line of its own output.
    if let Ok(mut attrs) = rustix::termios::tcgetattr(&controller) {
        attrs.local_modes -= rustix::termios::LocalModes::ECHO;
        let _ =
            rustix::termios::tcsetattr(&controller, rustix::termios::OptionalActions::Now, &attrs);
    }
    Ok((controller, OwnedFd::from(device)))
}

/// The descriptor a command's output is written to, apart from the terminal. The terminal is the
/// shell's: an interactive one redraws the line being typed, paints prompts, and may write a guess
/// at what comes next, none of which is the command's and none of which can be stripped reliably.
/// The shell opens this itself from a named pipe: inheriting a descriptor needs code between
/// fork and exec, which needs `unsafe`; `exec 3>` is POSIX.
const REPORT_FD: i32 = 3;

/// Make the fifo somewhere that will have it: the temporary directory, then the working directory.
/// A peer under `bwrap` with a read-only filesystem has no writable `/tmp`.
fn make_fifo() -> anyhow::Result<std::path::PathBuf> {
    sweep_stale_fifos();
    let name = format!("magi-shell-{}-{}", std::process::id(), nonce());
    let mut refused = None;
    for dir in [std::env::temp_dir(), std::path::PathBuf::from(".")] {
        let path = dir.join(&name);
        match rustix::fs::mknodat(
            rustix::fs::CWD,
            &path,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        ) {
            Ok(()) => return Ok(path),
            Err(why) => refused = Some(why),
        }
    }
    Err(refused.map_or_else(
        || anyhow::anyhow!("nowhere to put the shell's output channel"),
        anyhow::Error::from,
    ))
}

/// Remove the fifos of peers that are no longer running: one is left behind whenever a peer is
/// killed between telling its shell about the pipe and the shell opening it. The pid is in it.
fn sweep_stale_fifos() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name
            .to_string_lossy()
            .strip_prefix("magi-shell-")
            .map(|rest| rest.split('-').next().unwrap_or_default().to_owned())
        else {
            continue;
        };
        if !rest.is_empty() && !std::path::Path::new(&format!("/proc/{rest}")).exists() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A named pipe for one shell's output, and the lines that come out of it. Opening a fifo for
/// reading blocks until somebody opens it for writing, so the read happens on the thread that will
/// go on doing it. The path is removed as soon as both ends are open.
fn report_pipe() -> anyhow::Result<(std::path::PathBuf, Receiver<String>)> {
    let path = make_fifo()?;
    let (lines, incoming) = std::sync::mpsc::channel();
    let opening = path.clone();
    std::thread::spawn(move || {
        let Ok(pipe) = std::fs::File::open(&opening) else {
            return;
        };
        let _ = std::fs::remove_file(&opening);
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                // A closed receiver means this shell was abandoned; the thread outlives it only
                // until whatever still holds the descriptor lets go.
                Ok(_) => {
                    // A command writing to a terminal of its own ends lines with CRLF.
                    let cleaned = line.trim_end_matches(['\r', '\n']).to_owned();
                    if lines.send(cleaned).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Ok((path, incoming))
}

/// Spawn one shell, and a thread turning its output into lines.
fn spawn_shell(
    nonce: &str,
) -> anyhow::Result<(Child, std::fs::File, Receiver<String>, std::path::PathBuf)> {
    let (controller, device) = open_pty()?;
    // A terminal on all three. A shell writing to a pipe block-buffers, so nothing arrives until it
    // exits — and a shell like oslo reads a piped stdin to EOF before running any of it.
    let child = Command::new(shell_command())
        .env("TERM", "dumb")
        .stdin(Stdio::from(device.try_clone()?))
        .stdout(Stdio::from(device.try_clone()?))
        .stderr(Stdio::from(device))
        .spawn()?;

    // Two handles on our side: reading happens on its own thread, writing on this one.
    let mut to_shell = std::fs::File::from(controller.try_clone()?);
    let terminal = std::fs::File::from(controller);

    // Read and thrown away: a terminal nobody reads fills, and a shell writing into it stops.
    std::thread::spawn(move || {
        let mut sink = BufReader::new(terminal);
        let mut ignored = String::new();
        while sink.read_line(&mut ignored).is_ok_and(|read| read > 0) {
            ignored.clear();
        }
    });

    // The first two things the shell is told, before any command can be sent. A function rather
    // than three lines per call, because a shell with a history records everything it is fed.
    let (path, incoming) = report_pipe()?;
    writeln!(to_shell, "exec {REPORT_FD}>'{}'", path.display())?;
    writeln!(
        to_shell,
        "__magi() {{ printf '\\n__magi_{nonce}_%s__o\\n' \"$2\" >&{REPORT_FD}; \
         {{ eval \"$1\" ; __magi_status=$? ; }} < /dev/null >&{REPORT_FD} 2>&{REPORT_FD}; \
         printf '\\n__magi_{nonce}_%s__c%s\\n' \"$2\" \"$__magi_status\" >&{REPORT_FD}; }}"
    )?;
    to_shell.flush()?;
    Ok((child, to_shell, incoming, path))
}

impl Session {
    fn start() -> anyhow::Result<Self> {
        let nonce = nonce();
        let (child, stdin, lines, fifo) = spawn_shell(&nonce)?;
        Ok(Self {
            child,
            stdin,
            lines,
            fifo,
            interrupted: Arc::new(AtomicBool::new(false)),
            nonce,
            seq: 0,
            dead: false,
        })
    }

    /// Replace the shell, keeping the interrupt flag the reader thread already holds.
    fn restart(&mut self) -> anyhow::Result<()> {
        // A fresh marker, so a late line from the abandoned shell cannot end a command in this
        // one. Made before the shell, which bakes it into its helper.
        let nonce = nonce();
        let (child, stdin, lines, fifo) = spawn_shell(&nonce)?;
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.fifo);
        self.child = child;
        self.stdin = stdin;
        self.lines = lines;
        self.fifo = fifo;
        self.nonce = nonce;
        self.seq = 0;
        self.dead = false;
        Ok(())
    }

    /// Run one command and read until its marker, or until the host calls it off.
    fn run(&mut self, command: &str) -> (String, bool) {
        if self.dead && self.restart().is_err() {
            return ("the shell could not be restarted".to_owned(), true);
        }
        // A stop raised while nothing was running would cancel the next command instead.
        self.interrupted.store(false, Ordering::SeqCst);

        // stderr is folded into stdout for this command only, so ordering survives.
        self.seq += 1;
        let open = format!("{}o", marker(&self.nonce, self.seq));
        let close = format!("{}c", marker(&self.nonce, self.seq));
        // `< /dev/null` on the command group, because the shell's stdin IS this protocol's
        // control channel: a command that read stdin would eat the next command. Three lines,
        // with the command inside `eval`, so an empty or malformed command is a runtime error
        // whose message still comes back rather than a parse error that prints no marker.
        let quoted = command.replace('\'', r"'\''");
        let script = format!("__magi '{quoted}' {}\n", self.seq);
        if write!(self.stdin, "{script}").is_err() || self.stdin.flush().is_err() {
            return ("the shell is not accepting input".to_owned(), true);
        }

        let mut output = String::new();
        // What arrives on [`REPORT_FD`]: an OPEN marker, the output, then CLOSE<status>. The
        // open marker is waited for rather than assumed, because an interrupted command can
        // have left its own output in flight.
        let mut started = false;
        loop {
            if self.interrupted.swap(false, Ordering::SeqCst) {
                // Abandoned rather than waited out; anything the command spawned may outlive it.
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
                    if let Some(at) = line.find(&close) {
                        let code = line[at + close.len()..].trim_end();
                        let failed = code != "0";
                        // Each marker is printed after a newline of its own, so output ending
                        // without one does not share a line with it.
                        while output.ends_with('\n') {
                            output.pop();
                        }
                        if failed {
                            output.push_str(&format!("\n(exit {code})"));
                        }
                        return (output, failed);
                    }
                    output.push_str(&line);
                    output.push('\n');
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
        let _ = std::fs::remove_file(&self.fifo);
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
        // The reason this is a process rather than a function.
        let mut shell = Session::start().expect("a shell");
        shell.run("cd /tmp");
        let (output, _) = shell.run("pwd");
        assert_eq!(output.trim(), "/tmp");

        shell.run("export MAGI_TEST_VAR=carried");
        let (output, _) = shell.run("echo $MAGI_TEST_VAR");
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
        // `exit` is a legitimate thing to run, and a subshell would cost the persistence.
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
    fn a_call_with_no_command_in_it_is_answered_rather_than_waited_out() {
        // A call with no `command` argument made the whole script a syntax error, so neither
        // marker was printed: silence for the whole timeout.
        let mut shell = Session::start().expect("a shell");
        let (_, failed) = shell.run("");
        assert!(!failed, "an empty command is not a failure");
    }

    #[test]
    fn nothing_of_the_shell_own_screen_reaches_the_output() {
        // An interactive shell paints a prompt around every command; the output goes down a
        // descriptor of its own now, and this is what says so.
        let mut shell = Session::start().expect("a shell");
        let (output, _) = shell.run("echo only-this");
        assert_eq!(output, "only-this");
    }

    #[test]
    fn a_command_that_will_not_parse_says_why() {
        // Through `eval`, so the complaint is a runtime message rather than a parse error.
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("if then");
        assert!(failed, "it did not run");
        assert!(!output.is_empty(), "and it said something: {output:?}");
    }

    #[test]
    fn output_that_does_not_end_in_a_newline_still_ends_the_read() {
        // `cat` on a file with no trailing newline runs straight into the marker.
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("printf no-newline");
        assert_eq!(output, "no-newline");
        assert!(!failed);
    }

    #[test]
    fn a_command_cannot_counterfeit_the_end_of_its_own_output() {
        // The marker is found anywhere on a line, so it has to be one a command cannot guess.
        let mut shell = Session::start().expect("a shell");
        let (output, failed) = shell.run("echo '__magi_done__0'; echo after");
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
        // The shell's stdin is this protocol's control channel: `sort` reads to EOF, so before the
        // redirect it swallowed the end-of-command marker and the call hung.
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
        // `cat` echoed the marker back, so the exit status reported was the leaked line's.
        let mut shell = Session::start().expect("a shell");
        let (output, _) = shell.run("cat");
        assert!(
            !output.contains("magi-"),
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

/// Take a shell's own colour, title and shell-integration codes out of a line: none of it is the
/// command's. CSI sequences end at their final byte; OSC ones run to a BEL or an ST.
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
#[path = "shell/choosing.rs"]
mod choosing_tests;
