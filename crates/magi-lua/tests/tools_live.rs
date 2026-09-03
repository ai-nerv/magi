//! The shipped Lua tools, against siblings that are actually running.
//!
//! Skips quietly when nothing is listening: a test that needs someone else's daemon must not
//! fail a build on a machine that has none. When one *is* running, this is the only thing that
//! proves the client, the socket primitive and the tool declaration line up — every one of those
//! works when magi talks to itself.

use magi_lua::Engine;
use magi_tools::Registry;
use std::cell::RefCell;
use std::rc::Rc;

/// Read at run time, because the product does: nothing under `config/` is compiled in.
fn config(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn is_live(name: &str) -> bool {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::read_dir(runtime.join(name)).is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("api@"))
    })
}

#[test]
fn the_hexe_tool_answers_when_a_mux_is_running() {
    let mut engine = Engine::new();
    engine.install_clients(&[("hexe".to_owned(), config("clients/hexe.lua"))]);
    engine
        .run(&config("tools.lua"), "tools.lua")
        .expect("the tool declaration must run");

    let engine = Rc::new(RefCell::new(engine));
    let mut registry = Registry::new();
    magi_lua::tool::install(Rc::clone(&engine), &mut registry, &Default::default());

    assert!(registry.get("hexe").is_some(), "the tool registered");

    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    let output = registry.call(
        "hexe",
        &serde_json::json!({ "what": "verbs" }),
        &ops,
        &magi_tools::Uncancelled,
    );

    if !is_live("hexe") {
        // No mux: the tool must say so plainly rather than fail. A tool that errors when the
        // thing it asks about is simply absent teaches the model to stop asking.
        assert!(!output.is_error, "{}", output.content);
        assert!(
            output.content.contains("no hexe session"),
            "{}",
            output.content
        );
        return;
    }

    assert!(
        !output.is_error,
        "hexe is running but the tool failed: {}",
        output.content
    );
    assert!(
        output.content.contains("panes") || output.content.contains("verbs"),
        "the mux answered with something unexpected: {}",
        output.content
    );
    eprintln!(
        "hexe tool answered: {}",
        &output.content[..output.content.len().min(120)]
    );
}

#[test]
fn a_lua_tool_reports_an_absent_sibling_as_information_not_a_failure() {
    let mut engine = Engine::new();
    // No clients installed at all, which is what an install without `make configs` looks like.
    engine.run(&config("tools.lua"), "tools.lua").expect("run");
    let engine = Rc::new(RefCell::new(engine));
    let mut registry = Registry::new();
    magi_lua::tool::install(Rc::clone(&engine), &mut registry, &Default::default());

    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    let output = registry.call(
        "hexe",
        &serde_json::json!({}),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(output.is_error, "a missing client is a real problem");
    assert!(
        output.content.contains("make configs"),
        "{}",
        output.content
    );
}

/// A live `melchior serve`: the child, what it says, and the name it chose for itself.
///
/// The *reader* rather than the pipe, because what is already buffered is lost with the reader
/// that buffered it — and the line a test is waiting for may well be in there.
type Session = (
    std::process::Child,
    std::io::BufReader<std::process::ChildStdout>,
    String,
);

/// Start one, and wait until it is reachable.
///
/// The child is handed back rather than dropped: melchior exits when its parent's pipe closes, which
/// is the whole of how a session's socket lives exactly as long as the session. Letting it go
/// here would end it before the tool under test could reach it.
fn a_session(project: &str) -> Option<Session> {
    use std::io::BufRead;
    let mut child = std::process::Command::new("melchior")
        .args(["serve", "--project", project])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut said = String::new();
    let mut out = std::io::BufReader::new(child.stdout.take()?);
    out.read_line(&mut said).ok()?;
    let named = said
        .split("\"as\":\"")
        .nth(1)?
        .split('"')
        .next()?
        .to_owned();
    Some((child, out, named))
}

#[test]
fn the_agent_tool_reaches_another_session_through_melchior() {
    // The one path nothing else covers, and every piece of it works in isolation: the Lua
    // declaration, the placeholder substitution, the environment a tool is spawned with, and
    // melchior's own socket. It is the *join* that was wrong before -- the tool was a peer process
    // and became a command, and a command that inherited no environment would be a session that
    // cannot say who it is, refusing every verb with a plausible-sounding message.
    let project = format!("magi-test-{}", std::process::id());
    let Some((mut a, _hears_a, me)) = a_session(&project) else {
        eprintln!("melchior is not installed; skipping");
        return;
    };
    let Some((mut b, mut hears_b, them)) = a_session(&project) else {
        let _ = a.kill();
        eprintln!("melchior is not installed; skipping");
        return;
    };

    // What `magi_cli::host::stamp` puts on the backend every tool is spawned from. Spelled out
    // rather than imported: this crate cannot see the CLI, and a test that shared the code
    // would not notice the day the two stopped agreeing.
    let mut parts = me.split('/');
    let environ: std::collections::BTreeMap<String, String> = [
        ("MAGI_MELCHIOR_PROJECT", parts.next().unwrap_or_default()),
        ("MAGI_MELCHIOR_ROLE", parts.next().unwrap_or_default()),
        ("MAGI_MELCHIOR_ID", parts.next().unwrap_or_default()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();

    let mut engine = Engine::new();
    engine
        .run(&config("tools.lua"), "tools.lua")
        .expect("the tool declaration must run");
    let engine = Rc::new(RefCell::new(engine));
    let mut registry = Registry::new();
    magi_lua::tool::install(Rc::clone(&engine), &mut registry, &environ);
    assert!(
        registry.get("agent").is_some(),
        "the agent tool did not register"
    );

    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    // `list` takes no `who`, so its placeholder is filled with nothing. melchior drops an empty
    // argument rather than looking for an instance named "" -- which is the contract between a
    // substitution that must fill every flag and a program that must not be confused by it.
    let listed = registry.call(
        "agent",
        &serde_json::json!({ "verb": "list" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    let id = them.rsplit('/').next().unwrap_or_default().to_owned();
    assert!(!listed.is_error, "{}", listed.content);
    assert!(
        listed.content.contains(&id),
        "the other session is not in the list: {}",
        listed.content
    );

    let sent = registry.call(
        "agent",
        &serde_json::json!({ "verb": "send", "who": id, "message": "does this reach you" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!sent.is_error, "{}", sent.content);

    // And it arrived where a harness reads it: up the receiving session's own pipe, which is
    // the line magi turns into an entry in its transcript.
    let heard = {
        use std::io::BufRead;
        let mut line = String::new();
        // Past the roster it publishes when the second session appeared.
        while hears_b.read_line(&mut line).is_ok_and(|read| read > 0) {
            if line.contains("\"message\"") {
                break;
            }
            line.clear();
        }
        line
    };
    let _ = a.kill();
    let _ = b.kill();
    assert!(
        heard.contains("does this reach you") && heard.contains(&me),
        "the receiving session heard: {heard}"
    );
}

/// The argument vector the shipped `agent` declaration actually builds.
///
/// Rendered from `config/tools.lua` rather than a fixture, because the bug was *in* the
/// declaration and a fixture would have been written from the same misunderstanding.
fn agent_argv(call: serde_json::Value) -> Vec<String> {
    let mut engine = Engine::new();
    engine.run(&config("tools.lua"), "tools.lua").expect("runs");
    let declared = engine.tools();
    let spec = declared
        .iter()
        .find(|(name, _)| name == "agent")
        .map(|(_, spec)| spec.clone())
        .expect("the agent tool is declared");
    let args: Vec<String> = spec["transport"]["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|a| a.as_str().unwrap_or_default().to_owned())
        .collect();
    magi_tools::command::render(&args, &call)
}

#[test]
fn an_argument_the_model_left_out_takes_its_flag_with_it() {
    // The bug, exactly. An absent argument is dropped *whole* -- but only when the flag and the
    // placeholder are one token. Written as `"--about", "{about}"`, the placeholder vanished and
    // the bare flag stayed, so `reply` with no `about` sent `--about --sort` and the layer read
    // the next flag as the value: `about` came out as the string "--sort".
    let argv = agent_argv(serde_json::json!({ "verb": "list" }));
    assert_eq!(argv, vec!["tool", "--verb=list"], "{argv:?}");
}

#[test]
fn no_rendered_flag_is_ever_left_holding_the_next_one() {
    // The general form, so this cannot come back under a different argument name. Every token
    // after the subcommand carries its own value; none is a bare flag waiting to swallow one.
    for call in [
        serde_json::json!({ "verb": "help" }),
        serde_json::json!({ "verb": "inbox" }),
        serde_json::json!({ "verb": "status", "who": "beta-nu" }),
        serde_json::json!({ "verb": "send", "who": "beta-nu", "message": "hello" }),
        serde_json::json!({ "verb": "reply", "who": "beta-nu", "message": "yes", "about": "m1" }),
    ] {
        for token in agent_argv(call.clone()).iter().skip(1) {
            assert!(
                token.starts_with("--") && token.contains('='),
                "{token:?} is a bare flag and will take the next argument as its value: {call}"
            );
        }
    }
}

#[test]
fn what_the_model_sends_arrives_as_what_it_meant() {
    // The values themselves, because a `=` in a message must not split the pair: the name ends
    // at the *first* `=` and everything after it is the value.
    let argv = agent_argv(serde_json::json!({
        "verb": "reply",
        "who": "beta-nu",
        "message": "x = y + 1",
        "about": "m1",
    }));
    assert!(argv.contains(&"--message=x = y + 1".to_owned()), "{argv:?}");
    assert!(argv.contains(&"--about=m1".to_owned()), "{argv:?}");
}

#[test]
fn the_memory_tools_register_and_answer_when_balthasar_is_running() {
    let mut engine = Engine::new();
    engine.install_clients(&[("balthasar".to_owned(), config("clients/balthasar.lua"))]);
    engine
        .run(&config("tools.lua"), "tools.lua")
        .expect("the tool declaration must run");

    let engine = Rc::new(RefCell::new(engine));
    let mut registry = Registry::new();
    magi_lua::tool::install(Rc::clone(&engine), &mut registry, &Default::default());

    if !answers("balthasar") {
        // Nothing to register from: the vocabulary is balthasar's, so with balthasar absent
        // there are no memory tools rather than empty ones.
        assert!(
            registry.get("recall").is_none(),
            "declared without a source"
        );
        return;
    }

    for verb in ["recall", "remember", "forget", "why"] {
        assert!(registry.get(verb).is_some(), "{verb} did not register");
    }

    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    let phrase = format!("the wire held at {}", std::process::id());

    let kept = registry.call(
        "remember",
        &serde_json::json!({ "text": phrase }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!kept.is_error, "remember failed: {}", kept.content);

    let found = registry.call(
        "recall",
        &serde_json::json!({ "query": "wire held" }),
        &ops,
        &magi_tools::Uncancelled,
    );
    assert!(!found.is_error, "recall failed: {}", found.content);
    eprintln!(
        "recall answered: {}",
        &found.content[..found.content.len().min(200)]
    );
}

/// Whether a sibling is actually serving, rather than merely having left a socket behind.
///
/// A socket file outlives the process that bound it, so listing the directory answers "did one
/// run here" and not "is one running". Connecting is the only test that cannot be raced.
fn answers(name: &str) -> bool {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::read_dir(runtime.join(name)).is_ok_and(|dir| {
        dir.flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("api@"))
            .any(|e| std::os::unix::net::UnixStream::connect(e.path()).is_ok())
    })
}
