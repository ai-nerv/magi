//! `magi -p` and `magi --resume`, driving the real binary. Nothing is faked but melchior: a script
//! on the run's own `PATH` plays the sibling that owns the model. These need a real balthasar — it
//! is the store and there is no journal — so a checkout without it skips them, see
//! [`without_a_store`]. `PATH` goes on the child, the only place magi looks for a sibling.

use magi_model::scratch::Scratch;

use magi_testkit::Mind;
use magi_testkit::mind::{MODEL, call_lines, stop_line, text_line};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A working directory, with a machine config naming the fake melchior's one model. No provider and
/// no protocol: those are melchior's, and an opinion here would be a second catalog to keep in step.
fn workspace(name: &str) -> Scratch {
    let dir = Scratch::new("m1", name);
    std::fs::create_dir_all(dir.join("run")).expect("mkdir");
    std::fs::create_dir_all(dir.join("sessions")).expect("mkdir");
    install_config(&dir.join("config/magi"));
    // A setting, not a declaration: choosing among what exists carries no authority. Appended, not
    // replaced, because the entry point names what loads.
    let init = dir.join("config/magi/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str(&format!("\nmagi.model = \"{MODEL}\"\n"));
    std::fs::write(&init, source).expect("write init");
    dir
}

/// Run the binary under test in `dir`, isolated from the machine's own config and sockets. The
/// socket is named explicitly; use [`unpinned`] for the tests that are about the naming itself.
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

/// The command a run is, before it is waited on. Shared with the crash test, which needs the child.
fn started(dir: &Path, mind: &Mind, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    // The `XDG_` variables below do not settle which balthasar this reaches; `MAGI_API_SOCKET`
    // outranks them. See [`magi_testkit::only_its_own_store`].
    magi_testkit::only_its_own_store(&mut command);
    command
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        // The siblings keep what they are given under here, isolated as firmly as the config is.
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("PATH", ahead_of_path(dir, mind))
        .args(args);
    command
}

/// The fake melchior, then the workspace's stubs, then whatever `PATH` this runner has. In front
/// rather than instead: the shell tool runs real commands.
fn ahead_of_path(dir: &Path, mind: &Mind) -> String {
    let inherited = std::env::var("PATH").unwrap_or_default();
    format!(
        "{}:{}:{inherited}",
        mind.on_path().display(),
        dir.join("bin").display()
    )
}

/// A program that is not there, however installed the real one is. Left for the one test that
/// asserts magi refuses to run without a store.
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

/// Remove a workspace. A session is the process that shows it, so `magi` returning ends it.
fn teardown(_dir: &Path) {}

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

/// Each test here runs a magi, and every magi convenes a balthasar of its own — eight `balthasar
/// serve` processes racing to bind inside the start-up patience. The same lock `magi-host`'s live
/// tests take. Held for the body of the test, so the balthasar is gone before the next one starts.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take that lock, ignoring a poisoning left by a test that already failed.
fn alone() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the store this whole file depends on is installed. A checkout without balthasar skips
/// these rather than failing them, which is the convention `magi-host`'s live tests use.
fn without_a_store() -> bool {
    let missing = std::process::Command::new("balthasar")
        .arg("verbs")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err();
    if missing {
        eprintln!("skipped: balthasar is not installed, and it is the store");
    }
    missing
}

/// Every file magi wrote that looks like a transcript of its own, expected to be empty always: two
/// stores is one store and a copy that goes stale. Searched over the workspace rather than one
/// directory, because a second store reintroduced elsewhere would be as wrong and harder to notice.
fn transcripts_magi_wrote(dir: &Path) -> Vec<PathBuf> {
    sockets(dir)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|end| end == "jsonl"))
        .filter(|path| !path.starts_with(dir.join("data").join("balthasar")))
        .collect()
}

/// What balthasar kept for this run. The store lands under the run's own isolated `XDG_DATA_HOME`,
/// so a non-empty one is proof this session's history went somewhere, and the only place it could.
fn what_the_store_holds(dir: &Path) -> Vec<PathBuf> {
    sockets(&dir.join("data").join("balthasar"))
}

#[test]
fn print_mode_writes_the_answer_to_stdout_and_exits_zero() {
    let _alone = alone();
    if without_a_store() {
        return;
    }
    let dir = workspace("pr");
    let mind = Mind::answering("one-print", "append-only");
    let output = magi(&dir, &mind, &["-p", "what is it"]);

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
fn a_one_shot_records_into_balthasar_and_nowhere_else() {
    // The reason `-p` runs a session at all: the answer is recorded, not thrown away with the process.
    let _alone = alone();
    if without_a_store() {
        return;
    }
    let dir = workspace("jr");
    let mind = Mind::answering("one-journal", "recorded");
    let output = magi(&dir, &mind, &["-p", "say something"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !what_the_store_holds(&dir).is_empty(),
        "the session went nowhere: balthasar's store is empty"
    );
    let ours = transcripts_magi_wrote(&dir);
    assert!(
        ours.is_empty(),
        "magi kept a transcript of its own beside the store: {ours:?}"
    );
    teardown(&dir);
}

#[test]
fn resuming_reconstructs_the_context_the_model_is_given() {
    // The second invocation must send the first's exchange back to the model.
    let _alone = alone();
    if without_a_store() {
        return;
    }
    let dir = workspace("rs");
    let mind = Mind::answering("one-resume", "noted");
    assert!(
        magi(&dir, &mind, &["-p", "remember gerbil"])
            .status
            .success()
    );

    let second = magi(&dir, &mind, &["--resume", "-p", "and now?"]);
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // The proof is what the model was sent: the second ask carries the first exchange, so the model
    // is answering a conversation rather than a question that arrived out of nowhere.
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
    // The socket used to be named after the working directory, so a second `magi` started in the
    // same place attached to the first one's daemon. Sequential rather than concurrent, because what
    // is under test is the naming.
    let _alone = alone();
    if without_a_store() {
        return;
    }
    let dir = workspace("sp");
    let mind = Mind::answering("one-split", "a");
    for _ in 0..2 {
        let out = unpinned(&dir, &mind, &["-p", "hello"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // Counted in what the model was sent, not in files on disk. A second run that had joined the
    // first would carry its exchange, and `hello` would appear twice in the second ask.
    let asks = mind.asks();
    assert_eq!(asks.len(), 2, "one ask per run: {asks:?}");
    assert_eq!(
        asks[1].matches("hello").count(),
        1,
        "the second run joined the first's conversation: {}",
        asks[1]
    );

    // A socket file nobody is listening on is indistinguishable from a session that is merely busy.
    // magi's own, not everything under `run`: balthasar's endpoints are `lifecycle.rs`'s business.
    let left = magis_own_sockets(&dir);
    assert!(left.is_empty(), "something was left behind: {left:?}");
    teardown(&dir);
}

/// A melchior that asks for one tool and then answers, which is the shape every real tool-using
/// prompt has and the one no single-round fake can produce.
fn calling(name: &str, tool: &str, args: &str, answer: &str) -> Mind {
    let call = call_lines("c1", tool, args);
    let first: Vec<&str> = call.iter().map(String::as_str).collect();
    let said = text_line(answer);
    let stop = stop_line();
    Mind::turns(name, &[&first, &[&said, &stop]])
}

#[test]
fn print_mode_waits_for_the_answer_after_a_tool_runs() {
    let _alone = alone();
    if without_a_store() {
        return;
    }
    // A tool-using turn goes idle between rounds, and print mode took that idle for the end: it
    // exited zero having printed the empty message the model sent before reaching for the tool.
    let dir = workspace("tl");
    let mind = calling(
        "one-tooling",
        "read",
        "{\"path\":\"note.txt\"}",
        "two lines",
    );
    std::fs::write(dir.join("note.txt"), "alpha\nbeta\n").expect("write");
    let output = magi(&dir, &mind, &["-p", "count the lines"]);

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

/// Copy the checkout's `config/` into a test's config directory — the same thing `make configs` does
/// for a person. Without it every test fails identically at "no configuration".
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
    let _alone = alone();
    if without_a_store() {
        return;
    }
    // What replaced `magi stop`: the session is the process now, so returning from `magi` is the end
    // of it, with no socket, no pid file and no second process.
    let dir = workspace("nb");
    let mind = Mind::answering("one-nothing", "bye");
    let output = magi(&dir, &mind, &["-p", "what is it"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let left = magis_own_sockets(&dir);
    assert!(left.is_empty(), "{left:?}");
    teardown(&dir);
}

#[test]
fn a_permission_question_nobody_can_answer_ends_the_run_rather_than_hanging() {
    let _alone = alone();
    if without_a_store() {
        return;
    }
    // A `-p` run attaches, so the daemon has somebody to ask and stops the turn on the question.
    // Answered `Deny`, not `Allow`: a run nobody is watching is the wrong place to widen what a tool
    // may do, and `magi.allow` is how a person says in advance. A builtin deliberately — a test for
    // magi's own behaviour must not need a sibling installed.
    let dir = workspace("dq");
    let mind = calling(
        "one-declined",
        "write",
        "{\"path\":\"note.txt\",\"contents\":\"hi\"}",
        "I could not write it.",
    );
    let output = magi(&dir, &mind, &["-p", "write a note"]);

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
        "I could not write it.",
        "the model was told, and said so"
    );
    teardown(&dir);
}

#[test]
fn magi_refuses_to_run_without_the_store() {
    let _alone = alone();
    // There is one store now, and a session that cannot reach it does not start. Refusing is
    // affordable because magi convenes balthasar rather than finding it.
    let dir = workspace("ns");
    absent(&dir, "balthasar");
    let mind = Mind::answering("one-nostore", "never printed");
    let output = magi(&dir, &mind, &["-p", "say something"]);

    assert!(
        !output.status.success(),
        "magi ran with no store: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("balthasar"),
        "the refusal should name what is missing: {said}"
    );

    // And it wrote nothing of its own on the way out, which is the failure mode being closed.
    let ours = transcripts_magi_wrote(&dir);
    assert!(
        ours.is_empty(),
        "a refused session still left a store: {ours:?}"
    );
    teardown(&dir);
}

/// Sockets magi is responsible for, which is every one under `run` but the memory layer's own.
/// Both of its names, because it binds the role's and the one that was the program's. That it does
/// not outlive the magi that started it is held to in `lifecycle.rs`.
fn magis_own_sockets(dir: &Path) -> Vec<PathBuf> {
    let theirs = [dir.join("run").join("memory"), dir.join("run/balthasar")];
    sockets(&dir.join("run"))
        .into_iter()
        .filter(|path| !theirs.iter().any(|at| path.starts_with(at)))
        .collect()
}
