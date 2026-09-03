//! `magi -p` and `magi --resume`, driving the real binary.
//!
//! Nothing is faked but melchior: a script on the run's own `PATH` plays the sibling that owns
//! the model, and everything else — the spawned daemon, the socket, the Lua config, the journal
//! — is what a person invoking `magi` gets. The properties under test only exist at this level.
//! That a one-shot leaves a resumable session, and that resuming picks up this directory's
//! history, are claims about processes rather than about functions.
//!
//! `PATH` on the child rather than on the runner, because that is the only place magi looks for
//! a sibling and nothing in a config can point it elsewhere. A config that could name the
//! program that owns the model could name any program at all.

use magi_testkit::Mind;
use magi_testkit::mind::{MODEL, call_lines, stop_line, text_line};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A working directory, with a machine config naming the fake melchior's one model.
///
/// No provider and no protocol: which endpoint that model lives at and what credential it takes
/// are melchior's, and a config here that held an opinion about either would be a second
/// catalog to keep in step.
fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("magi-one-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).expect("mkdir");
    std::fs::create_dir_all(dir.join("sessions")).expect("mkdir");
    absent(&dir, "balthasar");
    install_config(&dir.join("config/magi"));
    // A setting, not a declaration: choosing among what exists carries no authority, so this
    // could equally have gone in the project file.
    // Appended, not replaced: the entry point names what loads, and a test that overwrote it
    // would be testing a config with no tools in it.
    let init = dir.join("config/magi/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str(&format!("\nmagi.model = \"{MODEL}\"\n"));
    std::fs::write(&init, source).expect("write init");
    dir
}

/// Run the binary under test in `dir`, isolated from the machine's own config and sockets.
///
/// The socket is named explicitly, so a test that only wants an answer gets a predictable
/// path. Use [`unpinned`] for the ones that are about the naming itself.
fn magi(dir: &Path, mind: &Mind, args: &[&str]) -> std::process::Output {
    let socket = dir.join("run/host.sock");
    let mut named: Vec<&str> = vec!["--socket", socket.to_str().expect("a path")];
    named.extend_from_slice(args);
    unpinned(dir, mind, &named)
}

/// The same, letting magi name its own socket the way it does for a person.
fn unpinned(dir: &Path, mind: &Mind, args: &[&str]) -> std::process::Output {
    started(dir, mind, args).output().expect("run magi")
}

/// The command a run is, before it is waited on.
///
/// Shared with the crash test, which needs the child rather than its output.
fn started(dir: &Path, mind: &Mind, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    command
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        // The siblings keep what they are given under here. Isolated as firmly as the config
        // is: a test that wrote to the machine's own data directory would be a test that
        // edits the person running it.
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("PATH", ahead_of_path(dir, mind))
        .args(args);
    command
}

/// The fake melchior, then the workspace's stubs, then whatever `PATH` this runner has.
///
/// In front rather than instead: the shell tool runs real commands, and a run with no `PATH`
/// but the fake would fail for a reason that has nothing to do with what is being tested.
fn ahead_of_path(dir: &Path, mind: &Mind) -> String {
    let inherited = std::env::var("PATH").unwrap_or_default();
    format!(
        "{}:{}:{inherited}",
        mind.on_path().display(),
        dir.join("bin").display()
    )
}

/// A program that is not there, however installed the real one is.
///
/// These tests are about the journal on disk, and a journal on disk is what a session has when
/// there is no memory layer: with balthasar reachable it *is* the store, and magi deliberately
/// keeps no second copy. Which of the two a checkout tests must not depend on what the person
/// running it happens to have installed, so the answer is pinned here rather than inherited.
fn absent(dir: &Path, program: &str) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    let path = bin.join(program);
    std::fs::write(&path, "#!/bin/sh\nexit 1\n").expect("write the stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
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

/// The journals a workspace has, which is one per session.
fn journals(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join("sessions"))
        .expect("sessions")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect()
}

#[test]
fn print_mode_writes_the_answer_to_stdout_and_exits_zero() {
    let dir = workspace("print");
    let mind = Mind::answering("one-print", "append-only");
    let output = magi(&dir, &mind, &["--sessions", "sessions", "-p", "what is it"]);

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
    let dir = workspace("journal");
    let mind = Mind::answering("one-journal", "recorded");
    let output = magi(
        &dir,
        &mind,
        &["--sessions", "sessions", "-p", "say something"],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let journals = journals(&dir);
    assert_eq!(journals.len(), 1, "{journals:?}");
    let source = std::fs::read_to_string(&journals[0]).expect("journal");
    assert!(source.contains("say something"), "the prompt was written");
    assert!(source.contains("recorded"), "the answer was written");
    teardown(&dir);
}

#[test]
fn resuming_reconstructs_the_context_the_model_is_given() {
    // Two separate invocations. The second must send the first's exchange back to the model,
    // which is the only externally visible proof that the context was rebuilt.
    let dir = workspace("resume");
    let mind = Mind::answering("one-resume", "noted");
    assert!(
        magi(
            &dir,
            &mind,
            &["--sessions", "sessions", "-p", "remember gerbil"]
        )
        .status
        .success()
    );

    let second = magi(
        &dir,
        &mind,
        &["--sessions", "sessions", "--resume", "-p", "and now?"],
    );
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let journals = journals(&dir);
    assert_eq!(
        journals.len(),
        1,
        "resumed rather than started: {journals:?}"
    );
    let source = std::fs::read_to_string(&journals[0]).expect("journal");
    assert!(source.contains("remember gerbil"), "the first prompt");
    assert!(source.contains("and now?"), "the second prompt, same file");

    // The point of resuming. The second ask carries the first exchange, so the model is
    // answering a conversation rather than a question that arrived out of nowhere.
    let asks = mind.asks();
    assert_eq!(asks.len(), 2, "one ask per run: {asks:?}");
    assert!(
        !asks[0].contains("and now?"),
        "the first run knew nothing of the second"
    );
    assert!(
        asks[1].contains("remember gerbil"),
        "the earlier prompt was replayed: {}",
        asks[1]
    );
    assert!(
        asks[1].contains("noted"),
        "and so was the answer to it: {}",
        asks[1]
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
    let dir = workspace("split");
    let mind = Mind::answering("one-split", "a");
    for _ in 0..2 {
        let out = unpinned(&dir, &mind, &["--sessions", "sessions", "-p", "hello"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let journals = journals(&dir);
    assert_eq!(journals.len(), 2, "one session each: {journals:?}");

    // And nothing outlives either of them. A socket file nobody is listening on is
    // indistinguishable from a session that is merely busy.
    let left = sockets(&dir.join("run"));
    assert!(left.is_empty(), "something was left behind: {left:?}");
    teardown(&dir);
}

#[test]
fn a_daemon_killed_mid_turn_leaves_a_journal_that_still_loads() {
    // The crash case. The prompt is journalled before the model is asked, so the record of
    // what was asked survives even though no answer ever came back.
    let dir = workspace("crash");
    let mind = Mind::silent("one-crash");
    let mut child = started(
        &dir,
        &mind,
        &[
            "--socket",
            dir.join("run/host.sock").to_str().expect("a path"),
            "--sessions",
            "sessions",
            "-p",
            "a question with no answer",
        ],
    )
    .stdout(std::process::Stdio::null())
    .spawn()
    .expect("run magi");

    // Wait for the prompt to reach the journal, which is what the kill has to interrupt.
    let mut journal = None;
    for _ in 0..100 {
        journal = journals(&dir).into_iter().find(|p| {
            std::fs::read_to_string(p).is_ok_and(|s| s.contains("a question with no answer"))
        });
        if journal.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let journal = journal.expect("the prompt was journalled before the model was asked");

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

/// A melchior that asks for one tool and then answers, which is the shape every real
/// tool-using prompt has and the one no single-round fake can produce.
fn calling(name: &str, tool: &str, args: &str, answer: &str) -> Mind {
    let call = call_lines("c1", tool, args);
    let first: Vec<&str> = call.iter().map(String::as_str).collect();
    let said = text_line(answer);
    let stop = stop_line();
    Mind::turns(name, &[&first, &[&said, &stop]])
}

#[test]
fn print_mode_waits_for_the_answer_after_a_tool_runs() {
    // Found against a real model. A tool-using turn goes idle between rounds — the model says
    // "tool_use", the tools run, the next round begins — and print mode took that idle for the
    // end. It exited zero, having printed the empty message the model sent before it reached
    // for the tool, which for most tool-using prompts is nothing at all.
    let dir = workspace("tooling");
    let mind = calling(
        "one-tooling",
        "read",
        "{\"path\":\"note.txt\"}",
        "two lines",
    );
    std::fs::write(dir.join("note.txt"), "alpha\nbeta\n").expect("write");
    let output = magi(
        &dir,
        &mind,
        &["--sessions", "sessions", "-p", "count the lines"],
    );

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
    let dir = workspace("nothing-behind");
    let mind = Mind::answering("one-nothing", "bye");
    let output = magi(&dir, &mind, &["--sessions", "sessions", "-p", "what is it"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let left = sockets(&dir.join("run"));
    assert!(left.is_empty(), "{left:?}");
    teardown(&dir);
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
    //
    // `shell` because it is gated on every call, which is the point: the question has to be
    // answered by somebody, and in `-p` there is nobody.
    let dir = workspace("declined");
    let mind = calling(
        "one-declined",
        "shell",
        "{\"command\":\"echo hi\"}",
        "I could not run it.",
    );
    let output = magi(
        &dir,
        &mind,
        &["--sessions", "sessions", "-p", "run echo hi"],
    );

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
    teardown(&dir);
}
