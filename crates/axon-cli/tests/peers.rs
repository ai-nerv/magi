//! Two peers, one protocol.
//!
//! A protocol with one implementation is a function call with extra steps: nothing forces the
//! host to say what it means, because the only peer was written alongside it. These run both
//! peers axon ships — a shell in `sh`, and Lua in its own VM — through the same registry, and
//! check the things that are only true if the wire is real: that the caller cannot tell them
//! apart, that they do not share state, and that a peer which cannot answer an interrupt is
//! still one the host can handle.

use axon_tools::{Registry, Uncancelled};
use std::path::PathBuf;

/// A Lua peer file, and a session with both peers registered.
fn session(name: &str, lua: &str) -> (Registry, axon_tools::ops::Real, PathBuf) {
    let (mut registry, ops, dir) = session_raw(name);
    std::fs::write(dir.join("peer.lua"), lua).expect("write");

    registry.register(Box::new(axon_tools::process::ProcessTool::new(
        "bash",
        "Run a shell command.",
        serde_json::json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"],
        }),
        env!("CARGO_BIN_EXE_axon"),
        vec!["ext".into(), "shell".into()],
    )));
    registry.register(Box::new(axon_tools::process::ProcessTool::new(
        "greet",
        "Say hello to somebody.",
        serde_json::json!({
            "type": "object",
            "properties": { "who": { "type": "string" } },
            "required": ["who"],
        }),
        env!("CARGO_BIN_EXE_axon"),
        vec![
            "ext".into(),
            "lua".into(),
            dir.join("peer.lua").display().to_string(),
        ],
    )));
    // As a daemon does, before its first turn. Until a peer has been asked, `parameters()` is
    // whatever the config claimed — and the registry now checks a call against the schema it
    // would have shown the model, so a session that never probes checks against the wrong one.
    registry.probe(&ops);
    (registry, ops, dir)
}

/// The directory and the builtins, with no peers registered yet.
fn session_raw(name: &str) -> (Registry, axon_tools::ops::Real, PathBuf) {
    let dir = std::env::temp_dir().join(format!("axon-peers-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut registry = Registry::new();
    axon_tools::builtin::install(&mut registry);
    (registry, axon_tools::ops::Real::new(dir.clone()), dir)
}

/// A peer file offering one tool.
const GREETER: &str = r#"
axon.tool("greet", {
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
axon.tool("greet", {
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
axon.tool("greet", {
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
impl axon_tools::Cancel for After {
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
axon.tool("greet", {
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
axon.tool("greet", {
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
axon.tool("greet", {
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
    registry.register(Box::new(axon_tools::process::ProcessTool::new(
        "greet",
        "A stale description nobody updated.",
        serde_json::json!({
            "type": "object",
            "properties": { "wrong": { "type": "number" } },
            "required": ["wrong"],
        }),
        env!("CARGO_BIN_EXE_axon"),
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
    let dir = std::env::temp_dir().join(format!("axon-peers-{}-silent", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut registry = Registry::new();
    registry.register(Box::new(axon_tools::process::ProcessTool::new(
        "absent",
        "What the config claimed.",
        serde_json::json!({ "type": "object" }),
        "/nonexistent/peer",
        Vec::new(),
    )));
    registry.probe(&axon_tools::ops::Real::new(dir.clone()));

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
    // Found against a real model. The shipped config named the peer `axon` and trusted PATH to
    // find it; PATH found an older install that did not know `ext`, which exited at once. All
    // the model was told was "io: Broken pipe (os error 32)", so it retried, failed the same
    // way, and went looking for the problem somewhere else entirely.
    let (mut registry, ops, dir) = session_raw("complaining");
    registry.register(Box::new(axon_tools::process::ProcessTool::new(
        "broken",
        "A peer that refuses its arguments.",
        serde_json::json!({ "type": "object" }),
        env!("CARGO_BIN_EXE_axon"),
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
    // `command = "axon"` resolves through PATH, so it finds whichever copy the shell sees --
    // an older install, or none. `axon.self` is the one actually running.
    // Read, not compiled in: the product reads its configuration and carries no copy.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/tools.lua");
    let source = std::fs::read_to_string(&path).expect("the checkout's tools");
    assert!(
        source.contains("command = axon.self"),
        "bash.lua must not rely on PATH"
    );
}

#[test]
fn a_peer_can_be_confined_by_configuration_alone() {
    // Open question 5, settled: axon needs no sandboxing subsystem because a process tool
    // names the command that starts its peer. Putting `bwrap` in front of that command is a
    // config change. Tau made namespaces mandatory and fail-closed and was punished for it on
    // every platform that has none; here it is the user's choice, and a machine without
    // `bwrap` uses the peer directly.
    let Ok(bwrap) = which("bwrap") else {
        eprintln!("no bwrap; the confinement check did not run");
        return;
    };
    let (mut registry, ops, dir) = session_raw("confined");
    registry.register(Box::new(axon_tools::process::ProcessTool::new(
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
            env!("CARGO_BIN_EXE_axon").to_owned(),
            "ext".into(),
            "shell".into(),
        ],
    )));

    let outside = registry.call(
        "bash",
        &serde_json::json!({ "command": "touch /etc/axon-should-not-exist" }),
        &ops,
        &Uncancelled,
    );
    assert!(
        outside.content.contains("Read-only") || outside.is_error,
        "writing outside the session directory is refused: {}",
        outside.content
    );
    assert!(
        !std::path::Path::new("/etc/axon-should-not-exist").exists(),
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

#[test]
fn a_round_of_calls_to_different_peers_overlaps() {
    // The milestone, measured. Two peers each sleeping a second: run one after the other that is
    // two seconds, and the point of sending both before collecting either is that it is one.
    //
    // `Tool` is deliberately not `Send` — a Lua tool runs in a VM that is not — so this cannot
    // be threads. It does not need to be: a peer is another *process*, and writing its request
    // and coming back for the answer is all the concurrency there is to have.
    let (mut registry, ops, dir) = session_raw("overlap");
    for name in ["one", "two"] {
        registry.register(Box::new(axon_tools::process::ProcessTool::new(
            name,
            "A shell.",
            serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"],
            }),
            env!("CARGO_BIN_EXE_axon"),
            vec!["ext".into(), "shell".into()],
        )));
    }
    registry.probe(&ops);
    // Started before the clock: the first call to a peer pays for spawning it, which is not
    // what this is measuring.
    for name in ["one", "two"] {
        let _ = registry.answer(name, r#"{"command":"true"}"#, &ops, &Uncancelled);
    }

    let started = std::time::Instant::now();
    let sent: Vec<_> = ["one", "two"]
        .into_iter()
        .map(|name| registry.prepare(name, r#"{"command":"sleep 1"}"#, &ops))
        .collect();
    assert!(
        sent.iter().all(axon_tools::Prepared::in_flight),
        "both went out"
    );
    for prepared in sent {
        let output = registry.finish(prepared, &ops, &Uncancelled);
        assert!(!output.is_error, "{}", output.content);
    }
    let took = started.elapsed();

    assert!(
        took < std::time::Duration::from_millis(1800),
        "two one-second calls took {took:?}, which is the sum rather than the slowest"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_calls_to_one_peer_still_take_their_turn() {
    // A peer answers one call at a time, so a second call to the *same* tool has nothing to
    // overlap with the first. It says so rather than pretending, and runs where it stands.
    let (mut registry, ops, dir) = session_raw("queued");
    registry.register(Box::new(axon_tools::process::ProcessTool::new(
        "bash",
        "A shell.",
        serde_json::json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"],
        }),
        env!("CARGO_BIN_EXE_axon"),
        vec!["ext".into(), "shell".into()],
    )));
    registry.probe(&ops);

    let first = registry.prepare("bash", r#"{"command":"echo one"}"#, &ops);
    let second = registry.prepare("bash", r#"{"command":"echo two"}"#, &ops);
    assert!(first.in_flight(), "the first went out");
    assert!(!second.in_flight(), "the second waits its turn");

    assert_eq!(
        registry.finish(first, &ops, &Uncancelled).content.trim(),
        "one"
    );
    assert_eq!(
        registry.finish(second, &ops, &Uncancelled).content.trim(),
        "two",
        "and still answers, in the order it was asked"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A peer is given the chance to clean up after itself, rather than shot where it stands.
mod goodbye {
    use super::*;

    /// Whether a process is still there.
    fn alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    /// Every process whose environment carries `mark`.
    ///
    /// Found by its environment rather than by parentage: cargo runs the tests of one binary as
    /// threads of one process, so every peer every other test started is a sibling of this
    /// one's and a scan by parent picks up whichever answered first.
    fn marked(mark: &str) -> Vec<u32> {
        let needle = format!("AXON_PEER_MARK={mark}");
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| {
                std::fs::read(format!("/proc/{pid}/environ"))
                    .map(|raw| {
                        String::from_utf8_lossy(&raw)
                            .split('\0')
                            .any(|pair| pair == needle)
                    })
                    .unwrap_or(false)
            })
            .collect()
    }

    /// The children of `pid`, by scanning `/proc`. Linux-only, which this project is.
    fn children_of(pid: u32) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(|each| {
                let Ok(stat) = std::fs::read_to_string(format!("/proc/{each}/stat")) else {
                    return false;
                };
                // The command name is field two, in parentheses, and may hold spaces.
                // Everything after the last `)` is fixed-width: state, then the parent.
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                    .and_then(|parent| parent.parse::<u32>().ok())
                    == Some(pid)
            })
            .collect()
    }

    #[test]
    fn the_shell_a_peer_started_goes_with_it() {
        // The defect: `drop_peer` sent SIGKILL and *then* ran `Peer::drop`, so the careful
        // goodbye -- close its stdin, give it a moment -- ran on a process that was already
        // dead. The shell peer never reached its own cleanup, so every session leaked the
        // shell it had opened. A machine running axon for a day carried a thousand of them,
        // parented to init, until `fork` started failing.
        let mark = format!("goodbye-{}", std::process::id());
        let (_, ops, _dir) = session_raw("goodbye");
        let mut registry = Registry::new();
        registry.register(Box::new(
            axon_tools::process::ProcessTool::new(
                "bash",
                "Run a shell command.",
                serde_json::json!({
                    "type": "object",
                    "properties": { "command": { "type": "string" } },
                    "required": ["command"],
                }),
                env!("CARGO_BIN_EXE_axon"),
                vec!["ext".into(), "shell".into()],
            )
            .with_env(
                [("AXON_PEER_MARK".to_owned(), mark.clone())]
                    .into_iter()
                    .collect(),
            ),
        ));

        let out = registry.call(
            "bash",
            &serde_json::json!({ "command": "echo hi" }),
            &ops,
            &Uncancelled,
        );
        assert!(!out.is_error, "the peer answered: {:?}", out.content);

        // The mark reaches the shell too -- it inherits the peer's environment -- so the peer is
        // the marked process that is also this process's child.
        let mine = children_of(std::process::id());
        let peers: Vec<u32> = marked(&mark)
            .into_iter()
            .filter(|pid| mine.contains(pid))
            .collect();
        assert_eq!(peers.len(), 1, "exactly this test's peer: {peers:?}");
        let peer = peers[0];
        let shell = children_of(peer);
        assert!(!shell.is_empty(), "the peer started a shell");

        drop(registry);

        // The peer is given a second to stop; its shell goes with it.
        for _ in 0..40 {
            if !alive(peer) && shell.iter().all(|pid| !alive(*pid)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let left: Vec<u32> = shell.into_iter().filter(|pid| alive(*pid)).collect();
        panic!("the peer's shell outlived it: {left:?}");
    }
}
