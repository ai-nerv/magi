//! `magi -p` and `magi --resume`, driving the real binary.
//!
//! Nothing is faked but the model: a local listener answers with recorded SSE, and
//! everything else — the spawned daemon, the socket, the Lua config, the journal — is what a
//! person invoking `magi` gets. The properties under test only exist at this level. That a
//! one-shot leaves a resumable session, and that resuming picks up this directory's history,
//! are claims about processes rather than about functions.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// One recorded answer, reused for every request the fake provider receives.
fn stream(text: &str) -> String {
    format!(
        "event: message_start\n\
         data: {{\"message\":{{\"usage\":{{\"input_tokens\":4,\"output_tokens\":0}}}}}}\n\n\
         event: content_block_start\n\
         data: {{\"index\":0,\"content_block\":{{\"type\":\"text\"}}}}\n\n\
         event: content_block_delta\n\
         data: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n\
         event: message_delta\n\
         data: {{\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":3}}}}\n\n"
    )
}

/// Answer every request with `body` until the test ends.
///
/// A loop rather than one shot: a daemon outlives the run that started it, so the second
/// `magi -p` against the same session is a second request to the same provider.
fn serve(body: String) -> String {
    serve_recording(body, Arc::new(Mutex::new(Vec::new())))
}

/// The same, keeping every request body it was sent.
///
/// What a resumed session actually sends the provider is not visible from the journal, the
/// socket or the exit code. It is visible here.
fn serve_recording(body: String, seen: Arc<Mutex<Vec<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { return };
            let body = body.clone();
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(socket.try_clone().expect("clone"));
                let mut line = String::new();
                let mut length = 0usize;
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                // The request body has to be drained before the response, or the client sees a
                // reset while it is still writing and reports a transport error.
                let mut sink = vec![0u8; length];
                let _ = std::io::Read::read_exact(&mut reader, &mut sink);
                if let Ok(mut seen) = seen.lock() {
                    seen.push(String::from_utf8_lossy(&sink).into_owned());
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// A working directory, and a machine config pointing at the fake provider.
///
/// The provider goes in the machine's own configuration rather than a project `.magi.lua`,
/// because a project file is not allowed to declare one — see `trust.rs`. That is also where a
/// real provider lives, so this is the arrangement being tested rather than a way around it.
fn workspace(name: &str, base_url: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("magi-one-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).expect("mkdir");
    std::fs::create_dir_all(dir.join("sessions")).expect("mkdir");
    install_config(&dir.join("config/magi"));
    std::fs::write(
        dir.join("config/magi/providers.lua"),
        format!(
            "magi.provider(\"fake\", {{\n\
             \x20 name = \"Fake\",\n\
             \x20 api = \"anthropic-messages\",\n\
             \x20 base_url = \"{base_url}\",\n\
             \x20 auth = {{ kind = \"none\" }},\n\
             \x20 models = {{ {{ id = \"m\", name = \"M\", context_window = 200000, max_tokens = 4096 }} }},\n\
             }})\n"
        ),
    )
    .expect("write providers");
    // A setting, not a declaration: choosing among what exists carries no authority, so this
    // could equally have gone in the project file.
    // Appended, not replaced: the entry point names what loads, and a test that overwrote it
    // would be testing a config with no protocols in it.
    let init = dir.join("config/magi/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str("\nmagi.model = \"fake/m\"\n");
    std::fs::write(&init, source).expect("write init");
    dir
}

/// Run the binary under test in `dir`, isolated from the machine's own config and sockets.
///
/// The socket is named explicitly, so a test that only wants an answer gets a predictable
/// path. Use [`unpinned`] for the ones that are about the naming itself.
fn magi(dir: &Path, args: &[&str]) -> std::process::Output {
    let socket = dir.join("run/host.sock");
    let mut named: Vec<&str> = vec!["--socket", socket.to_str().expect("a path")];
    named.extend_from_slice(args);
    unpinned(dir, &named)
}

/// The same, letting magi name its own socket the way it does for a person.
fn unpinned(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_magi"))
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .args(args)
        .output()
        .expect("run magi")
}

/// Remove a workspace.
///
/// It used to have to hunt down daemons first, by recorded pid, and assert that each had died.
/// There is nothing to hunt: a session is the process that shows it, so `magi` returning means
/// its session is already over.
fn teardown(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Every socket left under `dir`, at any depth.
fn sockets(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(sockets(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn print_mode_writes_the_answer_to_stdout_and_exits_zero() {
    let dir = workspace("print", &serve(stream("append-only")));
    let output = magi(&dir, &["--sessions", "sessions", "-p", "what is it"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "append-only"
    );
    teardown(&dir);
}

#[test]
fn a_one_shot_leaves_a_session_behind() {
    // The reason `-p` goes through a daemon at all: the answer is journalled, not thrown away
    // with the process that printed it.
    let dir = workspace("journal", &serve(stream("recorded")));
    let output = magi(&dir, &["--sessions", "sessions", "-p", "say something"]);
    assert!(output.status.success());

    let journals: Vec<PathBuf> = std::fs::read_dir(dir.join("sessions"))
        .expect("sessions")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    assert_eq!(journals.len(), 1, "{journals:?}");
    let source = std::fs::read_to_string(&journals[0]).expect("journal");
    assert!(source.contains("say something"), "the prompt was written");
    assert!(source.contains("recorded"), "the answer was written");
    teardown(&dir);
}

#[test]
fn resuming_reconstructs_the_context_the_model_is_given() {
    // Two separate invocations. The second must send the first's exchange back to the
    // provider, which is the only externally visible proof that the context was rebuilt.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dir = workspace(
        "resume",
        &serve_recording(stream("noted"), Arc::clone(&seen)),
    );
    assert!(
        magi(&dir, &["--sessions", "sessions", "-p", "remember gerbil"])
            .status
            .success()
    );

    let second = magi(
        &dir,
        &["--sessions", "sessions", "--resume", "-p", "and now?"],
    );
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let journals: Vec<PathBuf> = std::fs::read_dir(dir.join("sessions"))
        .expect("sessions")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    assert_eq!(
        journals.len(),
        1,
        "resumed rather than started: {journals:?}"
    );
    let source = std::fs::read_to_string(&journals[0]).expect("journal");
    assert!(source.contains("remember gerbil"), "the first prompt");
    assert!(source.contains("and now?"), "the second prompt, same file");

    // The point of resuming. The second request carries the first exchange, so the model is
    // answering a conversation rather than a question that arrived out of nowhere.
    let requests = seen.lock().expect("requests").clone();
    assert_eq!(requests.len(), 2, "one request per run");
    assert!(
        !requests[0].contains("and now?"),
        "the first run knew nothing of the second"
    );
    assert!(
        requests[1].contains("remember gerbil"),
        "the earlier prompt was replayed: {}",
        requests[1]
    );
    assert!(
        requests[1].contains("noted"),
        "and so was the answer to it: {}",
        requests[1]
    );
    teardown(&dir);
}

#[test]
fn two_runs_in_one_directory_do_not_share_a_session() {
    // The bug this is here for. The socket used to be named after the *working directory*, so a
    // second `magi` started in the same place found the first one's daemon already answering
    // and attached to it: two windows, one session, one transcript, and whatever either of them
    // typed appeared in both.
    //
    // Sequential rather than concurrent, because what is under test is the *naming*: if the
    // socket were the directory's, both runs would use one path and leave one journal.
    let dir = workspace("split", &serve(stream("a")));
    for _ in 0..2 {
        let out = unpinned(&dir, &["--sessions", "sessions", "-p", "hello"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let journals: Vec<PathBuf> = std::fs::read_dir(dir.join("sessions"))
        .expect("sessions dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    assert_eq!(journals.len(), 2, "one session each: {journals:?}");

    // And nothing outlives either of them. A socket file nobody is listening on is
    // indistinguishable from a session that is merely busy.
    let left = sockets(&dir.join("run"));
    assert!(left.is_empty(), "something was left behind: {left:?}");
    teardown(&dir);
}

/// A provider that accepts the request and never answers, leaving a turn in flight.
fn serve_silently() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for socket in listener.incoming() {
            // Kept rather than dropped: closing would end the turn on its own, which is not
            // the thing being tested.
            held.push(socket);
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn a_daemon_killed_mid_turn_leaves_a_journal_that_still_loads() {
    // The crash case. The prompt is journalled before the provider is called, so the record of
    // what was asked survives even though no answer ever came back.
    let dir = workspace("crash", &serve_silently());
    let mut child = Command::new(env!("CARGO_BIN_EXE_magi"))
        .current_dir(&dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .arg("--socket")
        .arg(dir.join("run/host.sock"))
        .args(["--sessions", "sessions", "-p", "a question with no answer"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("run magi");

    // Wait for the prompt to reach the journal, which is what the kill has to interrupt.
    let mut journal = None;
    for _ in 0..100 {
        journal = std::fs::read_dir(dir.join("sessions"))
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .find(|p| {
                std::fs::read_to_string(p).is_ok_and(|s| s.contains("a question with no answer"))
            });
        if journal.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let journal = journal.expect("the prompt was journalled before the provider was called");

    let _ = child.kill();
    let _ = child.wait();

    // Reopening is the test: a torn tail is truncated, a corrupt line is not, and this
    // distinguishes them where a `contains` check on the text could not.
    let reopened = magi_journal::Journal::open(
        &journal,
        magi_proto::SessionId::new("unused"),
        &dir.display().to_string(),
        0,
    )
    .expect("the journal still loads");
    assert!(
        reopened
            .entries()
            .iter()
            .any(|e| matches!(e, magi_proto::Entry::User { text, .. }
                if text == "a question with no answer")),
        "the prompt survived: {:?}",
        reopened.entries()
    );
    teardown(&dir);
}

/// A provider that asks for a tool once, then answers.
///
/// The shape every real tool-using prompt has, and the one no single-round fake can produce.
fn serve_tool_then(answer: &'static str) -> String {
    const HEAD: &str = "event: message_start\n\
        data: {\"message\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for (served, socket) in listener.incoming().enumerate() {
            let Ok(mut socket) = socket else { return };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(socket.try_clone().expect("clone"));
                let mut line = String::new();
                let mut length = 0usize;
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut sink = vec![0u8; length];
                let _ = std::io::Read::read_exact(&mut reader, &mut sink);

                let body = if served == 0 {
                    format!(
                        "{HEAD}event: content_block_start\n\
                         data: {{\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\
                         \"id\":\"c1\",\"name\":\"read\",\"input\":{{}}}}}}\n\n\
                         event: content_block_delta\n\
                         data: {{\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\
                         \"partial_json\":\"{{\\\"path\\\":\\\"note.txt\\\"}}\"}}}}\n\n\
                         event: message_delta\n\
                         data: {{\"delta\":{{\"stop_reason\":\"tool_use\"}},\
                         \"usage\":{{\"output_tokens\":5}}}}\n\n"
                    )
                } else {
                    format!(
                        "{HEAD}event: content_block_start\n\
                         data: {{\"index\":0,\"content_block\":{{\"type\":\"text\"}}}}\n\n\
                         event: content_block_delta\n\
                         data: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\
                         \"text\":\"{answer}\"}}}}\n\n\
                         event: message_delta\n\
                         data: {{\"delta\":{{\"stop_reason\":\"end_turn\"}},\
                         \"usage\":{{\"output_tokens\":5}}}}\n\n"
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn print_mode_waits_for_the_answer_after_a_tool_runs() {
    // Found against a real model. A tool-using turn goes idle between rounds — the provider
    // says "tool_use", the tools run, the next round begins — and print mode took that idle
    // for the end. It exited zero, having printed the empty message the model sent before it
    // reached for the tool, which for most tool-using prompts is nothing at all.
    let dir = workspace("tooling", &serve_tool_then("two lines"));
    std::fs::write(dir.join("note.txt"), "alpha\nbeta\n").expect("write");
    let output = magi(&dir, &["--sessions", "sessions", "-p", "count the lines"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "two lines",
        "the answer after the tool, not the silence before it"
    );
    teardown(&dir);
}

/// Copy the checkout's `config/` into a test's config directory.
///
/// The binary carries no configuration, so a test that isolates `XDG_CONFIG_HOME` has to install
/// one — the same thing `make configs` does for a person. Without it there is no entry point, and
/// every test fails identically at "no configuration".
fn install_config(into: &Path) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("mkdir");
        for entry in std::fs::read_dir(from).expect("read config") {
            let path = entry.expect("entry").path();
            let name = path.file_name().expect("named");
            if path.is_dir() {
                copy(&path, &to.join(name));
            } else {
                std::fs::copy(&path, to.join(name)).expect("copy");
            }
        }
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    copy(&source, into);
}

#[test]
fn a_session_leaves_nothing_running_behind_it() {
    // What replaced `magi stop`. A daemon owned the session and a UI quitting was a *detach*,
    // so nothing ever ended one and a week of work left a process per project — `magi stop`
    // existed only to clean up after that. The session is the process now, so returning from
    // `magi` is the end of it, with no socket, no pid file and no second process.
    let dir = workspace("nothing-behind", &serve(stream("bye")));
    let output = magi(&dir, &["--sessions", "sessions", "-p", "what is it"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let left = sockets(&dir.join("run"));
    assert!(left.is_empty(), "{left:?}");
    teardown(&dir);
}

/// A fake model that calls `shell`, then answers whatever it is told about the result.
///
/// `shell` is gated on every call, which is the point: the question has to be answered by
/// somebody, and in `-p` there is nobody.
fn serve_shell_then(answer: &'static str) -> String {
    const HEAD: &str = "event: message_start\n\
        data: {\"message\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for (served, socket) in listener.incoming().enumerate() {
            let Ok(mut socket) = socket else { return };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(socket.try_clone().expect("clone"));
                let mut line = String::new();
                let mut length = 0usize;
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut sink = vec![0u8; length];
                let _ = std::io::Read::read_exact(&mut reader, &mut sink);

                let body = if served == 0 {
                    format!(
                        "{HEAD}event: content_block_start\n\
                         data: {{\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\
                         \"id\":\"c1\",\"name\":\"shell\",\"input\":{{}}}}}}\n\n\
                         event: content_block_delta\n\
                         data: {{\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\
                         \"partial_json\":\"{{\\\"command\\\":\\\"echo hi\\\"}}\"}}}}\n\n\
                         event: message_delta\n\
                         data: {{\"delta\":{{\"stop_reason\":\"tool_use\"}},\
                         \"usage\":{{\"output_tokens\":5}}}}\n\n"
                    )
                } else {
                    format!(
                        "{HEAD}event: content_block_start\n\
                         data: {{\"index\":0,\"content_block\":{{\"type\":\"text\"}}}}\n\n\
                         event: content_block_delta\n\
                         data: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\
                         \"text\":\"{answer}\"}}}}\n\n\
                         event: message_delta\n\
                         data: {{\"delta\":{{\"stop_reason\":\"end_turn\"}},\
                         \"usage\":{{\"output_tokens\":5}}}}\n\n"
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn a_permission_question_nobody_can_answer_ends_the_run_rather_than_hanging() {
    // The defect: a `-p` run attaches, so the daemon has somebody to ask and stops the turn on
    // the question. Print mode ignored `PermissionAsked` and waited for events that could not
    // arrive, so the run hung until it was killed -- with the call committed to the journal,
    // `result: null`, and nothing on screen saying what it was waiting for.
    //
    // Answered `Deny`, not `Allow`: `-p` is what goes in a pipeline, and a run nobody is
    // watching is the wrong place to widen what a tool may do. `magi.allow` is how a person
    // says in advance what an unattended run may do.
    let dir = workspace("declined", &serve_shell_then("I could not run it."));
    let output = magi(&dir, &["--sessions", "sessions", "-p", "run echo hi"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not permitted"),
        "it should say why it stopped: {stderr}"
    );
    assert!(
        output.status.success(),
        "a refusal is an answer, not a crash: {stderr}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "I could not run it.",
        "the model was told, and said so"
    );
}
