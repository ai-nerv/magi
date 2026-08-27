//! `bash` across the process boundary, for real.
//!
//! Spawns the actual peer, speaks the actual protocol, and runs actual commands. The point is
//! that nothing here knows it is talking to another process: it calls a tool in a registry,
//! exactly as the turn loop does.

use axum_tools::Registry;
use axum_tools::ops::Real;
use axum_tools::process::ProcessTool;

/// The `bash` tool, pointed at the binary this test was built alongside.
fn bash() -> ProcessTool {
    ProcessTool::new(
        "bash",
        "Run a shell command.",
        serde_json::json!({ "type": "object" }),
        env!("CARGO_BIN_EXE_axum"),
        vec!["ext".to_owned(), "shell".to_owned()],
    )
}

fn session(name: &str) -> (Registry, Real, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("axum-bash-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut registry = Registry::new();
    axum_tools::builtin::install(&mut registry);
    registry.register(Box::new(bash()));
    (registry, Real::new(dir.clone()), dir)
}

#[test]
fn a_command_runs_in_another_process_and_comes_back() {
    let (registry, ops, dir) = session("basic");
    let output = registry.call(
        "bash",
        &serde_json::json!({ "command": "echo hello" }),
        &ops,
    );
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "hello");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_peer_starts_in_the_session_directory() {
    let (registry, ops, dir) = session("cwd");
    let output = registry.call("bash", &serde_json::json!({ "command": "pwd" }), &ops);
    assert!(output.content.contains("axum-bash-"), "{}", output.content);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn state_survives_between_calls_because_the_peer_does() {
    // The property a per-call spawn cannot give you, and the reason this is a process.
    let (registry, ops, dir) = session("state");
    let _ = registry.call(
        "bash",
        &serde_json::json!({ "command": "export CARRIED=yes" }),
        &ops,
    );
    let output = registry.call(
        "bash",
        &serde_json::json!({ "command": "echo $CARRIED" }),
        &ops,
    );
    assert_eq!(output.content.trim(), "yes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_command_is_a_result_the_model_can_read() {
    let (registry, ops, dir) = session("failing");
    let output = registry.call(
        "bash",
        &serde_json::json!({ "command": "echo attempted; false" }),
        &ops,
    );
    assert!(output.is_error);
    assert!(output.content.contains("attempted"), "{}", output.content);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_builtins_and_the_peer_share_one_directory() {
    // The seam holding: a file written by a Rust tool is visible to a command run by a peer.
    let (registry, ops, dir) = session("shared");
    let written = registry.call(
        "write",
        &serde_json::json!({ "path": "note.txt", "contents": "from a builtin\n" }),
        &ops,
    );
    assert!(!written.is_error, "{}", written.content);

    let output = registry.call(
        "bash",
        &serde_json::json!({ "command": "cat note.txt" }),
        &ops,
    );
    assert_eq!(output.content.trim(), "from a builtin");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_dies_is_restarted_on_the_next_call() {
    let (registry, ops, dir) = session("restart");
    let killed = registry.call("bash", &serde_json::json!({ "command": "exit 1" }), &ops);
    assert!(killed.is_error);

    let output = registry.call(
        "bash",
        &serde_json::json!({ "command": "echo alive" }),
        &ops,
    );
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "alive");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_downstream_knows_which_transport_it_used() {
    // read is Rust, bash is a process, and the registry answers both the same way.
    let (registry, ops, dir) = session("uniform");
    let _ = registry.call(
        "write",
        &serde_json::json!({ "path": "a", "contents": "x" }),
        &ops,
    );
    for name in ["read", "bash"] {
        let args = if name == "read" {
            serde_json::json!({ "path": "a" })
        } else {
            serde_json::json!({ "command": "cat a" })
        };
        let output = registry.call(name, &args, &ops);
        assert!(!output.is_error, "{name}: {}", output.content);
        assert!(output.content.contains('x'), "{name}: {}", output.content);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
