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

/// A Lua peer file, and a session rooted next to it.
fn session(name: &str, lua: &str) -> (Registry, axum_tools::ops::Real, PathBuf) {
    let dir = std::env::temp_dir().join(format!("axum-peers-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let file = dir.join("peer.lua");
    std::fs::write(&file, lua).expect("write");

    let mut registry = Registry::new();
    axum_tools::builtin::install(&mut registry);
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
        vec!["ext".into(), "lua".into(), file.display().to_string()],
    )));
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
