//! Two peers, one protocol.
//!
//! A protocol with one implementation is a function call with extra steps: nothing forces the
//! host to say what it means, because the only peer was written alongside it. These run both
//! peers axum ships — a shell in `sh`, and Lua in its own VM — through the same registry, and
//! check the things that are only true if the wire is real: that the caller cannot tell them
//! apart, that they do not share state, and that a peer which cannot answer an interrupt is
//! still one the host can handle.

use axum_tools::{Registry, Uncancelled};
use std::path::PathBuf;

/// A Lua peer file, and a session with both peers registered.
fn session(name: &str, lua: &str) -> (Registry, axum_tools::ops::Real, PathBuf) {
    let (mut registry, ops, dir) = session_raw(name);
    std::fs::write(dir.join("peer.lua"), lua).expect("write");

    registry.register(Box::new(axum_tools::process::ProcessTool::new(
        "bash",
        "Run a shell command.",
        serde_json::json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"],
        }),
        env!("CARGO_BIN_EXE_axum"),
        vec!["ext".into(), "shell".into()],
    )));
    registry.register(Box::new(axum_tools::process::ProcessTool::new(
        "greet",
        "Say hello to somebody.",
        serde_json::json!({
            "type": "object",
            "properties": { "who": { "type": "string" } },
            "required": ["who"],
        }),
        env!("CARGO_BIN_EXE_axum"),
        vec![
            "ext".into(),
            "lua".into(),
            dir.join("peer.lua").display().to_string(),
        ],
    )));
    (registry, ops, dir)
}

/// The directory and the builtins, with no peers registered yet.
fn session_raw(name: &str) -> (Registry, axum_tools::ops::Real, PathBuf) {
    let dir = std::env::temp_dir().join(format!("axum-peers-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut registry = Registry::new();
    axum_tools::builtin::install(&mut registry);
    (registry, axum_tools::ops::Real::new(dir.clone()), dir)
}

/// A peer file offering one tool.
const GREETER: &str = r#"
axum.tool("greet", {
  description = "Say hello to somebody.",
  parameters = { type = "object", properties = { who = { type = "string" } }, required = { "who" } },
  run = function(args)
    return "hello " .. tostring(args.who)
  end,
})
"#;

#[test]
fn two_peers_answer_the_same_way_a_builtin_does() {
    // The registry holds a Rust function, a shell in another process, and a Lua VM in a third.
    // Nothing at this level can tell which is which, and that is the whole design.
    let (registry, ops, dir) = session("uniform", GREETER);
    let _ = registry.call(
        "write",
        &serde_json::json!({ "path": "a", "contents": "x" }),
        &ops,
        &Uncancelled,
    );

    let cases = [
        ("read", serde_json::json!({ "path": "a" }), "x"),
        ("bash", serde_json::json!({ "command": "cat a" }), "x"),
        ("greet", serde_json::json!({ "who": "x" }), "x"),
    ];
    for (name, args, expected) in cases {
        let output = registry.call(name, &args, &ops, &Uncancelled);
        assert!(!output.is_error, "{name}: {}", output.content);
        assert!(
            output.content.contains(expected),
            "{name}: {}",
            output.content
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_raises_reports_it_rather_than_dying() {
    // A Lua tool that errors is a failed call, not a failed peer: the next call must work.
    let (registry, ops, dir) = session(
        "raising",
        r#"
axum.tool("greet", {
  description = "Raises.",
  parameters = { type = "object" },
  run = function(args)
    if args.who == "boom" then error("no") end
    return "hello " .. tostring(args.who)
  end,
})
"#,
    );

    let output = registry.call(
        "greet",
        &serde_json::json!({ "who": "boom" }),
        &ops,
        &Uncancelled,
    );
    assert!(output.is_error, "{}", output.content);

    let output = registry.call(
        "greet",
        &serde_json::json!({ "who": "again" }),
        &ops,
        &Uncancelled,
    );
    assert!(!output.is_error, "{}", output.content);
    assert!(output.content.contains("again"), "{}", output.content);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_peers_do_not_share_state() {
    // Separate processes, so the shell's working directory is invisible to the Lua peer and
    // neither can reach into the other. A single peer could not show this.
    let (registry, ops, dir) = session(
        "isolated",
        r#"
axum.tool("greet", {
  description = "Reports what it can see.",
  parameters = { type = "object" },
  run = function() return "lua peer" end,
})
"#,
    );
    let _ = registry.call(
        "bash",
        &serde_json::json!({ "command": "cd /" }),
        &ops,
        &Uncancelled,
    );

    let shell = registry.call(
        "bash",
        &serde_json::json!({ "command": "pwd" }),
        &ops,
        &Uncancelled,
    );
    assert_eq!(shell.content.trim(), "/", "the shell kept its own cwd");

    let lua = registry.call("greet", &serde_json::json!({}), &ops, &Uncancelled);
    assert_eq!(lua.content.trim(), "lua peer");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An interrupt raised partway through, as `esc` is.
struct After(std::time::Instant);
impl axum_tools::Cancel for After {
    fn is_cancelled(&self) -> bool {
        std::time::Instant::now() >= self.0
    }
}

#[test]
fn a_peer_that_cannot_answer_an_interrupt_is_still_stopped() {
    // The Lua peer runs its body to completion inside a stackless VM and cannot notice a
    // `Cancel`. The host does not depend on it noticing: it asks, waits a bounded time, and
    // then stops waiting. Without a second peer there would be nothing that exercises this,
    // because the shell peer always answers.
    let (registry, ops, dir) = session(
        "deaf",
        r#"
axum.tool("greet", {
  description = "Never finishes.",
  parameters = { type = "object" },
  run = function()
    local n = 0
    while true do n = n + 1 end
  end,
})
"#,
    );

    let started = std::time::Instant::now();
    let output = registry.call(
        "greet",
        &serde_json::json!({}),
        &ops,
        &After(started + std::time::Duration::from_millis(300)),
    );
    let took = started.elapsed();

    assert!(
        took < std::time::Duration::from_secs(30),
        "the host waited {took:?} on a peer that was never going to answer"
    );
    assert!(output.is_error, "{}", output.content);
    assert!(
        output.content.contains("acknowledge"),
        "the result says what happened: {}",
        output.content
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_killed_for_ignoring_an_interrupt_is_replaced() {
    // The host kills a peer that will not stop, so the next call has to get a fresh one.
    let (registry, ops, dir) = session(
        "replaced",
        r#"
axum.tool("greet", {
  description = "Loops once, then behaves.",
  parameters = { type = "object" },
  run = function(args)
    if args.who == "forever" then
      local n = 0
      while true do n = n + 1 end
    end
    return "recovered"
  end,
})
"#,
    );
    let started = std::time::Instant::now();
    let _ = registry.call(
        "greet",
        &serde_json::json!({ "who": "forever" }),
        &ops,
        &After(started + std::time::Duration::from_millis(300)),
    );

    let output = registry.call(
        "greet",
        &serde_json::json!({ "who": "after" }),
        &ops,
        &Uncancelled,
    );
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "recovered");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_lua_peer_is_sandboxed_like_every_other_vm() {
    // Being in its own process is isolation, not permission. If a Lua peer could spawn, the
    // process transport would be a stylistic preference rather than the only way to run a
    // command -- which is the opposite of the design.
    let (registry, ops, dir) = session(
        "sandbox",
        r#"
axum.tool("greet", {
  description = "Tries to escape.",
  parameters = { type = "object" },
  run = function()
    if os.execute then return "os.execute is reachable" end
    if io then return "io is reachable" end
    return "sealed"
  end,
})
"#,
    );
    let output = registry.call("greet", &serde_json::json!({}), &ops, &Uncancelled);
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(output.content.trim(), "sealed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_peer_declares_and_the_config_is_only_a_claim() {
    // A config can only describe what somebody believed a program did when they wrote the
    // line. The peer is the thing that knows, so what the model is told comes from the peer --
    // and the schema below, which is wrong on purpose, is not what reaches it.
    let (mut registry, ops, dir) = session_raw("declared");
    std::fs::write(dir.join("peer.lua"), GREETER).expect("write");
    registry.register(Box::new(axum_tools::process::ProcessTool::new(
        "greet",
        "A stale description nobody updated.",
        serde_json::json!({
            "type": "object",
            "properties": { "wrong": { "type": "number" } },
            "required": ["wrong"],
        }),
        env!("CARGO_BIN_EXE_axum"),
        vec![
            "ext".into(),
            "lua".into(),
            dir.join("peer.lua").display().to_string(),
        ],
    )));
    registry.probe(&ops);

    let declared = registry
        .declarations()
        .into_iter()
        .find(|d| d.name == "greet")
        .expect("greet is registered");
    assert_eq!(
        declared.description, "Say hello to somebody.",
        "the peer's description won"
    );
    assert!(
        declared.parameters["properties"].get("who").is_some(),
        "the peer's schema won: {}",
        declared.parameters
    );
    assert!(
        declared.parameters["properties"].get("wrong").is_none(),
        "the config's claim is gone: {}",
        declared.parameters
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_declares_nothing_leaves_the_config_claim_standing() {
    // The peer never starts, so nothing corrects the claim. Better than refusing to offer the
    // tool at all, and no worse than where things stood before it was asked.
    let dir = std::env::temp_dir().join(format!("axum-peers-{}-silent", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut registry = Registry::new();
    registry.register(Box::new(axum_tools::process::ProcessTool::new(
        "absent",
        "What the config claimed.",
        serde_json::json!({ "type": "object" }),
        "/nonexistent/peer",
        Vec::new(),
    )));
    registry.probe(&axum_tools::ops::Real::new(dir.clone()));

    let declared = registry
        .declarations()
        .into_iter()
        .find(|d| d.name == "absent")
        .expect("still registered");
    assert_eq!(declared.description, "What the config claimed.");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_cannot_start_says_why() {
    // Found against a real model. The shipped config named the peer `axum` and trusted PATH to
    // find it; PATH found an older install that did not know `ext`, which exited at once. All
    // the model was told was "io: Broken pipe (os error 32)", so it retried, failed the same
    // way, and went looking for the problem somewhere else entirely.
    let (mut registry, ops, dir) = session_raw("complaining");
    registry.register(Box::new(axum_tools::process::ProcessTool::new(
        "broken",
        "A peer that refuses its arguments.",
        serde_json::json!({ "type": "object" }),
        env!("CARGO_BIN_EXE_axum"),
        vec!["ext".into(), "lua".into(), "/nonexistent/peer.lua".into()],
    )));

    let output = registry.call("broken", &serde_json::json!({}), &ops, &Uncancelled);
    assert!(output.is_error, "{}", output.content);
    assert!(
        output.content.contains("nonexistent") || output.content.contains("loading"),
        "the peer's own complaint reaches the result: {}",
        output.content
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_shipped_config_names_the_binary_that_is_running() {
    // `command = "axum"` resolves through PATH, so it finds whichever copy the shell sees --
    // an older install, or none. `axum.self` is the one actually running.
    let source = include_str!("../../../config/tools/bash.lua");
    assert!(
        source.contains("command = axum.self"),
        "bash.lua must not rely on PATH"
    );
}

#[test]
fn a_peer_can_be_confined_by_configuration_alone() {
    // Open question 5, settled: axum needs no sandboxing subsystem because a process tool
    // names the command that starts its peer. Putting `bwrap` in front of that command is a
    // config change. Tau made namespaces mandatory and fail-closed and was punished for it on
    // every platform that has none; here it is the user's choice, and a machine without
    // `bwrap` uses the peer directly.
    let Ok(bwrap) = which("bwrap") else {
        eprintln!("no bwrap; the confinement check did not run");
        return;
    };
    let (mut registry, ops, dir) = session_raw("confined");
    registry.register(Box::new(axum_tools::process::ProcessTool::new(
        "bash",
        "A confined shell.",
        serde_json::json!({ "type": "object" }),
        &bwrap,
        vec![
            "--ro-bind".into(),
            "/".into(),
            "/".into(),
            "--dev".into(),
            "/dev".into(),
            "--proc".into(),
            "/proc".into(),
            "--bind".into(),
            dir.display().to_string(),
            dir.display().to_string(),
            env!("CARGO_BIN_EXE_axum").to_owned(),
            "ext".into(),
            "shell".into(),
        ],
    )));

    let outside = registry.call(
        "bash",
        &serde_json::json!({ "command": "touch /etc/axum-should-not-exist" }),
        &ops,
        &Uncancelled,
    );
    assert!(
        outside.content.contains("Read-only") || outside.is_error,
        "writing outside the session directory is refused: {}",
        outside.content
    );
    assert!(
        !std::path::Path::new("/etc/axum-should-not-exist").exists(),
        "and really did not happen"
    );

    let inside = registry.call(
        "bash",
        &serde_json::json!({ "command": "touch ./allowed && echo ok" }),
        &ops,
        &Uncancelled,
    );
    assert!(!inside.is_error, "{}", inside.content);
    assert_eq!(inside.content.trim(), "ok", "the session directory works");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Find a program on `PATH`, or say there is none.
fn which(program: &str) -> Result<String, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .map(|found| found.display().to_string())
        .ok_or(())
}
