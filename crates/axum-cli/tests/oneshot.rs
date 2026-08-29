//! `axum -p` and `axum --resume`, driving the real binary.
//!
//! Nothing is faked but the model: a local listener answers with recorded SSE, and
//! everything else — the spawned daemon, the socket, the Lua config, the journal — is what a
//! person invoking `axum` gets. The properties under test only exist at this level. That a
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
/// `axum -p` against the same session is a second request to the same provider.
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
/// The provider goes in the machine's own configuration rather than a project `.axum.lua`,
/// because a project file is not allowed to declare one — see `trust.rs`. That is also where a
/// real provider lives, so this is the arrangement being tested rather than a way around it.
fn workspace(name: &str, base_url: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axum-one-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).expect("mkdir");
    std::fs::create_dir_all(dir.join("sessions")).expect("mkdir");
    install_config(&dir.join("config/axum"));
    std::fs::write(
        dir.join("config/axum/providers.lua"),
        format!(
            "axum.provider(\"fake\", {{\n\
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
    let init = dir.join("config/axum/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str("\naxum.model = \"fake/m\"\n");
    std::fs::write(&init, source).expect("write init");
    dir
}

/// Run the binary under test in `dir`, isolated from the machine's own config and sockets.
fn axum(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_axum"))
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .arg("--socket")
        .arg(dir.join("run/host.sock"))
        .args(args)
        .output()
        .expect("run axum")
}

/// Stop the daemon this test started, so it does not outlive the run.
fn teardown(dir: &Path) {
    stop_daemon(dir);
    let _ = std::fs::remove_dir_all(dir);
}

/// Kill every daemon started under `dir`, and wait for its socket to stop answering.
///
/// By recorded pid, never by `pkill -f`: that pattern is matched against every command line on
/// the machine, and a temporary directory path is a prefix other processes can share -- the
/// one running these tests included.
fn stop_daemon(dir: &Path) {
    for pid_file in pid_files(&dir.join("run")) {
        let Ok(pid) = std::fs::read_to_string(&pid_file) else {
            continue;
        };
        let _ = Command::new("kill").arg(pid.trim()).status();

        let socket = pid_file.with_extension("sock");
        let stopped = (0..40).any(|_| {
            if std::os::unix::net::UnixStream::connect(&socket).is_err() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            false
        });
        // Asserted rather than tolerated: a surviving daemon still has the session open, so
        // the next run would attach to it and pass a resume test without resuming anything.
        assert!(stopped, "the daemon outlived the run that started it");
        let _ = std::fs::remove_file(&pid_file);
    }
}

/// Pid files directly under `dir` or one level below it, which is where both socket layouts
/// these tests use put them: an explicit `--socket`, and the default named for the directory.
fn pid_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(pid_files(&path));
        } else if path.extension().is_some_and(|e| e == "pid") {
            out.push(path);
        }
    }
    out
}

#[test]
fn print_mode_writes_the_answer_to_stdout_and_exits_zero() {
    let dir = workspace("print", &serve(stream("append-only")));
    let output = axum(&dir, &["--sessions", "sessions", "-p", "what is it"]);

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
    let output = axum(&dir, &["--sessions", "sessions", "-p", "say something"]);
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
        axum(&dir, &["--sessions", "sessions", "-p", "remember gerbil"])
            .status
            .success()
    );
    stop_daemon(&dir);

    let second = axum(
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
fn two_directories_do_not_share_a_session() {
    // The default socket is named for the working directory, so `axum` in one project cannot
    // attach to another's transcript.
    let one = workspace("split-a", &serve(stream("a")));
    let two = workspace("split-b", &serve(stream("b")));
    let a = Command::new(env!("CARGO_BIN_EXE_axum"))
        .current_dir(&one)
        .env("XDG_RUNTIME_DIR", one.join("run"))
        .env("XDG_CONFIG_HOME", one.join("config"))
        .args(["--sessions", "sessions", "-p", "hello"])
        .output()
        .expect("run");
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));

    let sockets: Vec<PathBuf> = std::fs::read_dir(one.join("run/axum"))
        .expect("runtime dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sock"))
        .collect();
    assert_eq!(sockets.len(), 1, "{sockets:?}");
    assert_ne!(
        sockets[0].file_name(),
        Some(std::ffi::OsStr::new("host.sock")),
        "the socket is named for the directory, not shared"
    );
    // Without this the teardown below has nothing to kill and says nothing about it, which is
    // how the default socket layout leaked a daemon per run without any test noticing.
    assert!(
        !pid_files(&one.join("run")).is_empty(),
        "the daemon recorded where to find it"
    );
    teardown(&one);
    teardown(&two);
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_axum"))
        .current_dir(&dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .arg("--socket")
        .arg(dir.join("run/host.sock"))
        .args(["--sessions", "sessions", "-p", "a question with no answer"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("run axum");

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

    stop_daemon(&dir);
    let _ = child.kill();
    let _ = child.wait();

    // Reopening is the test: a torn tail is truncated, a corrupt line is not, and this
    // distinguishes them where a `contains` check on the text could not.
    let reopened = axum_journal::Journal::open(
        &journal,
        axum_proto::SessionId::new("unused"),
        &dir.display().to_string(),
        0,
    )
    .expect("the journal still loads");
    assert!(
        reopened
            .entries()
            .iter()
            .any(|e| matches!(e, axum_proto::Entry::User { text, .. }
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
    let output = axum(&dir, &["--sessions", "sessions", "-p", "count the lines"]);

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

#[test]
fn stop_ends_the_daemon_and_takes_its_files_with_it() {
    // Quitting the UI is a detach on purpose, so nothing else ever ends a daemon. Without
    // this, a week of work leaves a process per project and `ps | grep` is the interface.
    let dir = workspace("stop", &serve(stream("hello")));
    assert!(
        axum(&dir, &["--sessions", "sessions", "-p", "hi"])
            .status
            .success()
    );
    let socket = dir.join("run/host.sock");
    assert!(
        std::os::unix::net::UnixStream::connect(&socket).is_ok(),
        "a daemon is running"
    );

    let output = axum(&dir, &["stop"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Stopped 1"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );

    assert!(
        std::os::unix::net::UnixStream::connect(&socket).is_err(),
        "and is gone"
    );
    // A socket file nobody is listening on is indistinguishable from a busy daemon, and the
    // next run waits out its whole startup timeout on one.
    assert!(!socket.exists(), "the socket file went with it");
    assert!(
        !dir.join("run/host.pid").exists(),
        "and so did the pid file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stopping_when_nothing_runs_says_so_rather_than_failing() {
    let dir = workspace("stop-nothing", &serve(stream("unused")));
    let output = axum(&dir, &["stop"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("No daemon"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let _ = std::fs::remove_dir_all(&dir);
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
fn the_daemon_runs_under_the_axum_profile() {
    // The peer sets this for itself, which covers the shell. The daemon needs it too, for
    // everything it starts that is not a peer — the `git` it asks about the branch, the `sh` a
    // permission check runs — or half of what a session does falls outside the profile it is
    // supposed to be recording under.
    let dir = workspace("profile", &serve(stream("here")));
    let output = axum(&dir, &["--sessions", "sessions", "-p", "what is it"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let running: Vec<u32> = pid_files(&dir.join("run"))
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|pid| pid.trim().parse::<u32>().ok())
        .collect();
    assert!(!running.is_empty(), "a daemon was started");

    for pid in running {
        let environ = std::fs::read(format!("/proc/{pid}/environ")).expect("its environment");
        let seen = String::from_utf8_lossy(&environ);
        assert!(
            seen.split('\0').any(|pair| pair == "OSLO_PROFILE=axum"),
            "daemon {pid} is outside the profile"
        );
    }
    teardown(&dir);
}

#[test]
fn a_daemon_nobody_is_attached_to_stops_on_its_own() {
    // The leak this closes: a `-p` run attaches, gets its answer and detaches, and the daemon
    // stayed up for the rest of the afternoon. One per directory per session is how twenty-two
    // of them end up running. Detaching still is not ending a session — the grace period is
    // what makes reattaching possible — so the test sets a short one rather than none.
    let dir = workspace("idle", &serve(stream("bye")));
    let init = dir.join("config/axum/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the entry point");
    source.push_str("\naxum.idle_exit = 1\n");
    std::fs::write(&init, source).expect("write");

    let output = axum(&dir, &["--sessions", "sessions", "-p", "what is it"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let started: Vec<u32> = pid_files(&dir.join("run"))
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|pid| pid.trim().parse::<u32>().ok())
        .collect();
    assert!(!started.is_empty(), "a daemon was started");

    // Its own clock, so this waits for the behaviour rather than for a fixed sleep.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let alive = |pid: u32| std::path::Path::new(&format!("/proc/{pid}")).exists();
    while std::time::Instant::now() < deadline && started.iter().copied().any(alive) {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let stragglers: Vec<u32> = started.iter().copied().filter(|p| alive(*p)).collect();
    assert!(
        stragglers.is_empty(),
        "still running with nobody attached: {stragglers:?}"
    );
    teardown(&dir);
}
