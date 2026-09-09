//! The shipped Lua tools, against siblings that are actually running.
//!
//! Skips quietly when nothing is listening. When one is running, this is the only thing that
//! proves the client, the socket primitive and the tool declaration line up.

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

// `hexe` and the missing-client case are casper's now, along with the tool itself. What is left is
// what magi still declares: `agent`, and the memory tools balthasar registers into this session.

/// A live `melchior serve`: the child, what it says, and the name it chose. The reader rather than
/// the pipe, because what is already buffered is lost with the reader that buffered it.
type Session = (
    std::process::Child,
    std::io::BufReader<std::process::ChildStdout>,
    String,
);

/// Do something on another thread, and give up on it after `patience`.
///
/// `read_line` on a `melchior serve` that is up and says nothing has no other end: the child is
/// held open for the whole test, so a line that goes astray blocks rather than fails. The deadline
/// is a backstop, not an assertion. The thread is left blocked when it fires.
fn within<T: Send + 'static>(
    patience: std::time::Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (done, ready) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(work()));
    ready.recv_timeout(patience).ok()
}

const READ_WITHIN: std::time::Duration = std::time::Duration::from_secs(20);

/// One of those, killed when the test ends rather than on its last line: `let _ = a.kill()` at the
/// bottom does not run when an `assert!` unwinds past it, and a `kill` with no `wait` leaves a
/// zombie.
struct Running(std::process::Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The project melchior files these sessions under, and the directory it keeps for it. Nothing
/// here can point melchior at a scratch — it reads `$XDG_RUNTIME_DIR` from its own environment and
/// `set_var` is `unsafe` — so the directory is removed afterwards, from a `Drop`.
struct Project(String);

impl std::ops::Deref for Project {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::remove_dir_all(runtime.join("melchior").join(&self.0));
    }
}

/// Start one, and wait until it is reachable. The child is handed back rather than dropped:
/// melchior exits when its parent's pipe closes, which is how a session's socket lives exactly as
/// long as the session.
fn a_session(project: &str) -> Option<Session> {
    use std::io::BufRead;
    let mut child = std::process::Command::new("melchior")
        .args(["serve", "--project", project])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let child_out = child.stdout.take()?;
    // The reader goes to the thread and comes back with the line, so the buffering survives the
    // deadline.
    let (said, out) = within(READ_WITHIN, move || {
        let mut said = String::new();
        let mut out = std::io::BufReader::new(child_out);
        let read = out.read_line(&mut said).is_ok();
        (read.then_some(said), out)
    })?;
    let said = said?;
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
    // Declared before the sessions so it drops after them: locals go in reverse order, and a
    // directory removed while a melchior is still running comes straight back.
    let project = Project(format!("magi-test-{}", std::process::id()));
    let Some((a, _hears_a, me)) = a_session(&project) else {
        eprintln!("melchior is not installed; skipping");
        return;
    };
    let _a = Running(a);
    let Some((b, mut hears_b, them)) = a_session(&project) else {
        eprintln!("melchior is not installed; skipping");
        return;
    };
    let _b = Running(b);

    // What `magi_cli::host::stamp` puts on the backend every tool is spawned from. Spelled out
    // rather than imported: this crate cannot see the CLI.
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
    // `list` takes no `who`, so its placeholder is filled with nothing, and melchior drops an empty
    // argument rather than looking for an instance named "".
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

    // And it arrived where a harness reads it: up the receiving session's own pipe.
    let heard = within(READ_WITHIN, move || {
        use std::io::BufRead;
        let mut line = String::new();
        while hears_b.read_line(&mut line).is_ok_and(|read| read > 0) {
            if line.contains("\"message\"") {
                break;
            }
            line.clear();
        }
        line
    })
    .unwrap_or_else(|| panic!("the receiving session said nothing within {READ_WITHIN:?}"));
    assert!(
        heard.contains("does this reach you") && heard.contains(&me),
        "the receiving session heard: {heard}"
    );
}

/// The argument vector the shipped `agent` declaration actually builds. Rendered from
/// `config/tools.lua` rather than a fixture, because the bug was in the declaration.
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
fn every_argument_the_agent_declares_reaches_melchior() {
    // `role` was declared by melchior and by nothing here, so `role` and `assign` -- the two verbs
    // that say what an agent is for -- were refused for want of an argument the model had no way
    // to send. The rule is the general one: an argument offered to the model and not passed on is
    // an argument that does nothing.
    let mut engine = Engine::new();
    engine.run(&config("tools.lua"), "tools.lua").expect("runs");
    let spec = engine
        .tools()
        .into_iter()
        .find(|(name, _)| name == "agent")
        .map(|(_, spec)| spec)
        .expect("the agent tool is declared");
    let args = spec["transport"]["args"].to_string();
    for name in spec["parameters"]["properties"]
        .as_object()
        .expect("properties")
        .keys()
    {
        assert!(
            args.contains(&format!("{{{name}}}")),
            "the model may send {name:?} and no argument carries it: {args}"
        );
    }
}

#[test]
fn an_argument_the_model_left_out_takes_its_flag_with_it() {
    // An absent argument is dropped whole, but only when the flag and the placeholder are one
    // token: written as `"--about", "{about}"`, the bare flag stayed and swallowed the next one.
    let argv = agent_argv(serde_json::json!({ "verb": "list" }));
    assert_eq!(argv, vec!["tool", "--verb=list"], "{argv:?}");
}

#[test]
fn no_rendered_flag_is_ever_left_holding_the_next_one() {
    // The general form: every token after the subcommand carries its own value, and none is a bare
    // flag waiting to swallow one.
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
    // A `=` in a message must not split the pair: the name ends at the first `=`.
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
    // From balthasar, not from a copy here: a vendored copy that had fallen behind silently
    // removed every memory tool from every session on a machine.
    let Some(client) = borrowed("balthasar") else {
        eprintln!("skipping: balthasar is not installed");
        return;
    };
    let mut engine = Engine::new();
    engine.install_clients(&[("balthasar".to_owned(), client)]);
    engine
        .run(&config("tools.lua"), "tools.lua")
        .expect("the tool declaration must run");

    let engine = Rc::new(RefCell::new(engine));
    let mut registry = Registry::new();
    magi_lua::tool::install(Rc::clone(&engine), &mut registry, &Default::default());

    if !answers("balthasar") {
        // The vocabulary is balthasar's, so with balthasar absent there are no memory tools.
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
/// A socket file outlives the process that bound it, and connecting is not enough either: the
/// kernel's backlog accepts for a listener whose owner has stopped reading. So it asks — one
/// `verbs` call, framed the way the family frames everything, answered within a moment.
fn answers(name: &str) -> bool {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let Ok(dir) = std::fs::read_dir(runtime.join(name)) else {
        return false;
    };
    dir.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("api@"))
        .any(|e| replies(&e.path()))
}

fn replies(socket: &std::path::Path) -> bool {
    use std::io::{Read, Write};

    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket) else {
        return false;
    };
    if stream.set_read_timeout(Some(PATIENCE)).is_err()
        || stream.set_write_timeout(Some(PATIENCE)).is_err()
    {
        return false;
    }
    let body = br#"{"call":"verbs"}"#;
    let mut framed = (body.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(body);
    if stream.write_all(&framed).is_err() {
        return false;
    }
    // The length alone: that four bytes came back at all is what says somebody is reading.
    let mut head = [0_u8; 4];
    stream.read_exact(&mut head).is_ok() && u32::from_be_bytes(head) > 0
}

/// How long a live sibling gets to answer one question. Generous: a local socket, answered from
/// memory, and still short enough that a wedged sibling does not hold the suite.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(1);

/// A sibling's client library, from the sibling. `None` when it is not installed or has none to
/// lend, which is a skip rather than a failure.
fn borrowed(program: &str) -> Option<String> {
    let out = std::process::Command::new(program)
        .arg("client")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let source = String::from_utf8(out.stdout).ok()?;
    // A library is Lua; a refusal is the family's reply shape, and no Lua chunk starts with `{`.
    (!source.trim_start().is_empty() && !source.trim_start().starts_with('{')).then_some(source)
}
