//! The shipped Lua tools, against siblings that are actually running.
//!
//! Skips quietly when nothing is listening: a test that needs someone else's daemon must not
//! fail a build on a machine that has none. When one *is* running, this is the only thing that
//! proves the client, the socket primitive and the tool declaration line up — every one of those
//! works when axon talks to itself.

use axon_lua::Engine;
use axon_tools::Registry;
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
    axon_lua::tool::install(Rc::clone(&engine), &mut registry, &Default::default());

    assert!(registry.get("hexe").is_some(), "the tool registered");

    let ops = axon_tools::ops::Real::new(std::env::temp_dir());
    let output = registry.call(
        "hexe",
        &serde_json::json!({ "what": "verbs" }),
        &ops,
        &axon_tools::Uncancelled,
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
    axon_lua::tool::install(Rc::clone(&engine), &mut registry, &Default::default());

    let ops = axon_tools::ops::Real::new(std::env::temp_dir());
    let output = registry.call(
        "hexe",
        &serde_json::json!({}),
        &ops,
        &axon_tools::Uncancelled,
    );
    assert!(output.is_error, "a missing client is a real problem");
    assert!(
        output.content.contains("make configs"),
        "{}",
        output.content
    );
}

/// A live `atom serve`: the child, what it says, and the name it chose for itself.
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
/// The child is handed back rather than dropped: atom exits when its parent's pipe closes, which
/// is the whole of how a session's socket lives exactly as long as the session. Letting it go
/// here would end it before the tool under test could reach it.
fn a_session(project: &str) -> Option<Session> {
    use std::io::BufRead;
    let mut child = std::process::Command::new("atom")
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
fn the_agent_tool_reaches_another_session_through_atom() {
    // The one path nothing else covers, and every piece of it works in isolation: the Lua
    // declaration, the placeholder substitution, the environment a tool is spawned with, and
    // atom's own socket. It is the *join* that was wrong before -- the tool was a peer process
    // and became a command, and a command that inherited no environment would be a session that
    // cannot say who it is, refusing every verb with a plausible-sounding message.
    let project = format!("axon-test-{}", std::process::id());
    let Some((mut a, _hears_a, me)) = a_session(&project) else {
        eprintln!("atom is not installed; skipping");
        return;
    };
    let Some((mut b, mut hears_b, them)) = a_session(&project) else {
        let _ = a.kill();
        eprintln!("atom is not installed; skipping");
        return;
    };

    // What `axon_cli::host::stamp` puts on the backend every tool is spawned from. Spelled out
    // rather than imported: this crate cannot see the CLI, and a test that shared the code
    // would not notice the day the two stopped agreeing.
    let mut parts = me.split('/');
    let environ: std::collections::BTreeMap<String, String> = [
        ("ATOM_PROJECT", parts.next().unwrap_or_default()),
        ("ATOM_ROLE", parts.next().unwrap_or_default()),
        ("ATOM_ID", parts.next().unwrap_or_default()),
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
    axon_lua::tool::install(Rc::clone(&engine), &mut registry, &environ);
    assert!(
        registry.get("agent").is_some(),
        "the agent tool did not register"
    );

    let ops = axon_tools::ops::Real::new(std::env::temp_dir());
    // `list` takes no `who`, so its placeholder is filled with nothing. atom drops an empty
    // argument rather than looking for an instance named "" -- which is the contract between a
    // substitution that must fill every flag and a program that must not be confused by it.
    let listed = registry.call(
        "agent",
        &serde_json::json!({ "verb": "list" }),
        &ops,
        &axon_tools::Uncancelled,
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
        &axon_tools::Uncancelled,
    );
    assert!(!sent.is_error, "{}", sent.content);

    // And it arrived where a harness reads it: up the receiving session's own pipe, which is
    // the line axon turns into an entry in its transcript.
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
