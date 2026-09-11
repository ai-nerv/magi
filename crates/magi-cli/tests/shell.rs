//! The shell peer across the process boundary, for real.
//!
//! Spawns the actual peer, speaks the actual protocol, and runs actual commands. The point is
//! that nothing here knows it is talking to another process: it calls a tool in a registry,
//! exactly as the turn loop does.

use magi_model::scratch::Scratch;

use magi_tools::Registry;
use magi_tools::ops::Real;
use magi_tools::process::ProcessTool;

/// The shell tool, pointed at the binary this test was built alongside.
fn shell_tool() -> ProcessTool {
    ProcessTool::new(
        "shell",
        "Run a shell command.",
        serde_json::json!({ "type": "object" }),
        env!("CARGO_BIN_EXE_magi"),
        vec!["ext".to_owned(), "shell".to_owned()],
    )
}

fn session(name: &str) -> (Registry, Real, Scratch) {
    let dir = Scratch::new("magi-bash", name);
    let mut registry = Registry::new();
    magi_tools::builtin::install_spawn(&mut registry, &Default::default());
    registry.register(Box::new(shell_tool()));
    (registry, Real::new(dir.to_path_buf()), dir)
}

#[test]
fn a_command_runs_in_another_process_and_comes_back() {
    let (registry, ops, _dir) = session("basic");
    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "echo hello" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "hello");
}

#[test]
fn the_peer_starts_in_the_session_directory() {
    let (registry, ops, _dir) = session("cwd");
    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "pwd" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(output.content.contains("magi-bash-"), "{}", output.content);
}

#[test]
fn state_survives_between_calls_because_the_peer_does() {
    // The property a per-call spawn cannot give you, and the reason this is a process.
    let (registry, ops, _dir) = session("state");
    let _ = registry.call(
        "shell",
        &serde_json::json!({ "command": "export CARRIED=yes" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "echo $CARRIED" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert_eq!(output.content.trim(), "yes");
}

#[test]
fn a_failing_command_is_a_result_the_model_can_read() {
    let (registry, ops, _dir) = session("failing");
    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "echo attempted; false" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(output.is_error);
    assert!(output.content.contains("attempted"), "{}", output.content);
}

#[test]
fn the_peer_shares_one_directory_across_calls() {
    // The seam holding: a file a command writes is there for the next command the peer runs.
    let (registry, ops, _dir) = session("shared");
    let written = registry.call(
        "shell",
        &serde_json::json!({ "command": "echo from a command > note.txt" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!written.is_error, "{}", written.content);

    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "cat note.txt" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert_eq!(output.content.trim(), "from a command");
}

#[test]
fn a_peer_that_dies_is_restarted_on_the_next_call() {
    let (registry, ops, _dir) = session("restart");
    let killed = registry.call(
        "shell",
        &serde_json::json!({ "command": "exit 1" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(killed.is_error);

    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "echo alive" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "alive");
}

// That a process peer and another transport answer the same way — nothing downstream knowing which
// it used — is tested against two live peers in `peers.rs`; magi has no Rust file tool to compare a
// peer against any more.

/// An interrupt the host has already decided on.
struct Stopped;
impl magi_tools::Cancel for Stopped {
    fn is_cancelled(&self) -> bool {
        true
    }
}

/// An interrupt that arrives partway through, as `esc` does.
struct After(std::time::Instant);
impl magi_tools::Cancel for After {
    fn is_cancelled(&self) -> bool {
        std::time::Instant::now() >= self.0
    }
}

/// A command nothing can wait out, which writes down the pid of what is actually running.
///
/// A minute is deliberate: the interrupt tests assert the call came back inside twenty seconds,
/// so a command that could finish first would let them pass without the interrupt working. The
/// `$!` half is what makes the sleep reapable — see [`Runaway`].
const LONG: &str = "sleep 60 & echo $! > runaway; wait";

/// The `sleep` an interrupt leaves behind, ended when the test ends.
///
/// **Interrupting kills the shell peer, and the peer is not the process running the command.**
/// `magi_cli::shell` says so where it does it, and calls anything the command spawned outliving
/// it the honest cost of interrupting something mid-flight. For a person that is a minute of a
/// runaway build. For this suite it was two orphaned `sleep 60`s per run, holding a scratch
/// directory open after it had been deleted — the same shape as `lifecycle`'s stand-in, which
/// wrote down the shell's pid and forked the thing that mattered, and leaked ten minutes at a
/// time for months because everything anybody looked at was the process that was named.
///
/// The pid is checked before it is signalled. It was read out of a file rather than handed over
/// by a spawn, and a pid nobody owns is not one to send `SIGKILL` at.
struct Runaway(std::path::PathBuf);

impl Drop for Runaway {
    fn drop(&mut self) {
        let Ok(text) = std::fs::read_to_string(self.0.join("runaway")) else {
            return;
        };
        let pid = text.trim();
        let Ok(said) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            return;
        };
        if !said.starts_with(b"sleep\0") {
            return;
        }
        let _ = std::process::Command::new("kill")
            .arg("-9")
            .arg(pid)
            .status();
    }
}

#[test]
fn a_running_command_is_interrupted_rather_than_waited_out() {
    // The point of the boundary. `sleep 60` is running in another process, and the message
    // asking it to stop has to reach a peer that is inside the command it is being asked to
    // abandon. Nothing here waits sixty seconds.
    let (registry, ops, _dir) = session("cancel");
    // After the directory, so it drops before it: the pid it needs is in a file in there.
    let _runaway = Runaway(_dir.to_path_buf());
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);

    let started = std::time::Instant::now();
    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": LONG }),
        &ops,
        &After(deadline),
    );
    let took = started.elapsed();

    assert!(
        took < std::time::Duration::from_secs(20),
        "the call returned in {took:?}, so it waited the command out"
    );
    assert!(output.is_error, "{}", output.content);
    assert!(
        output.content.contains("interrupted"),
        "the result says what happened: {}",
        output.content
    );
}

#[test]
fn the_peer_is_usable_again_after_an_interrupt() {
    // The shell is killed to interrupt it, so the next call has to get a fresh one rather than
    // an error about a process that is no longer there.
    let (registry, ops, _dir) = session("after-cancel");
    let _runaway = Runaway(_dir.to_path_buf());
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    let _ = registry.call(
        "shell",
        &serde_json::json!({ "command": LONG }),
        &ops,
        &After(deadline),
    );

    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "echo recovered" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "recovered");
}

#[test]
fn a_call_made_under_an_interrupt_does_not_run_forever() {
    // Cancelled before it began. The peer is told at the first opportunity rather than after
    // the poll interval decides the call is worth starting.
    let (registry, ops, _dir) = session("pre-cancel");
    // Nothing should run at all here, so the file should not appear. The guard is kept anyway:
    // "the command never started" is the claim, and a guard is how a broken claim gets tidied.
    let _runaway = Runaway(_dir.to_path_buf());
    let started = std::time::Instant::now();
    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": LONG }),
        &ops,
        &Stopped,
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert!(output.is_error, "{}", output.content);
}

#[test]
fn every_command_sees_the_magi_profile() {
    // The chain is peer -> shell -> command, and each link inherits from the one before, so
    // setting this where the peer is started is what reaches the command a tool actually runs.
    let (registry, ops, _dir) = session("profile");
    let output = registry.call(
        "shell",
        &serde_json::json!({ "command": "printf %s \"$OSLO_PROFILE\"" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "magi");
}
