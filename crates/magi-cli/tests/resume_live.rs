//! `--resume` when balthasar is the store, against a balthasar that is really there.
//!
//! The existing resume test runs with no memory layer, where the journal on disk is the record
//! and resuming means reading a file back. This is the other arrangement, and it is the one a
//! person with balthasar installed actually gets: **there is no journal at all**. balthasar holds
//! the transcript, the session id is asked for over a socket, and the entries come back through
//! `resume`. Nothing about that path is exercised by reading a file.
//!
//! Skipped when balthasar is not installed, the way the other `*_live` tests are. It cannot be
//! faked: the claim is that a *separate program* still has the conversation after the process
//! that had it is gone, and a stand-in that returned the right entries would be asserting that
//! this test knows what it wants to see.

use magi_model::scratch::Scratch;

use magi_testkit::Mind;
use magi_testkit::mind::MODEL;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether there is a balthasar to test against.
///
/// Looked for the way magi looks for it — a program on `PATH` — rather than at a build path, so
/// what is tested is the one a person would get.
fn installed() -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .any(|dir| Path::new(dir).join("balthasar").exists())
}

/// A workspace with no fake balthasar in front of the real one.
///
/// **A checkout**, because that is what a person runs magi in and because it is what makes each
/// of these a scope of its own. Without it they shared one: balthasar scopes a directory that is
/// not a checkout by walking up for one, and a stray `.git` high above — this machine had one in
/// the temporary directory and another in the home directory — collects every directory beneath
/// it. Four tests then ran four sessions into a single store, and `--resume` picked up whichever
/// had most recently written to it.
///
/// **Short names**, because a unix socket path may not exceed `SUN_LEN` — 108 bytes. The
/// directory's own name becomes the project and the project appears *inside* the socket path, so
/// a descriptive name here is spent twice. Under `gate-hermetic`, which nests the whole run in a
/// private temporary directory, that is what pushes it over: these passed alone and failed under
/// the gate, which is the arrangement the gate exists to catch.
fn workspace(name: &str) -> Scratch {
    let dir = Scratch::new("mr", name);
    for under in ["run", "sessions", "data", ".git"] {
        std::fs::create_dir_all(dir.join(under)).expect("mkdir");
    }
    install_config(&dir.join("config/magi"));
    let init = dir.join("config/magi/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str(&format!(
        "\nmagi.model = \"{MODEL}\"\nmagi.project = \"p\"\n"
    ));
    std::fs::write(&init, source).expect("write init");
    dir
}

/// The shipped configuration, copied where `magi` would find an installed one.
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
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config"),
        into,
    );
}

/// Run the binary in `dir`, with the fake melchior in front of a real `PATH`.
fn magi(dir: &Path, mind: &Mind, args: &[&str]) -> std::process::Output {
    let inherited = std::env::var("PATH").unwrap_or_default();
    Command::new(env!("CARGO_BIN_EXE_magi"))
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("PATH", format!("{}:{inherited}", mind.on_path().display()))
        .args(args)
        .output()
        .expect("run magi")
}

/// Every journal file the runs left behind.
fn journals(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect()
}

#[test]
fn a_second_run_picks_up_the_conversation_balthasar_kept() {
    if !installed() {
        eprintln!("skipping: no balthasar on PATH");
        return;
    }
    let dir = workspace("kept");
    let mind = Mind::answering("resume-kept", "noted");

    let first = magi(
        &dir,
        &mind,
        &["--sessions", "sessions", "-p", "remember gerbil"],
    );
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    // A separate process, after the first is entirely gone — along with the balthasar it
    // started, which is the point. What survives is the store, not a running thing.
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

    // The only externally visible proof: what the second run sent to the model. A resume that
    // found nothing still answers, and answers plausibly, so the reply says nothing at all.
    let asks = mind.asks();
    assert_eq!(asks.len(), 2, "one ask per run: {asks:?}");
    assert!(
        !asks[0].contains("and now?"),
        "the first run knew nothing of the second"
    );
    assert!(
        asks[1].contains("remember gerbil"),
        "the earlier prompt came back out of balthasar: {}",
        asks[1]
    );
    assert!(
        asks[1].contains("noted"),
        "and so did the answer to it: {}",
        asks[1]
    );
}

#[test]
fn with_balthasar_holding_it_there_is_no_journal_on_disk() {
    if !installed() {
        eprintln!("skipping: no balthasar on PATH");
        return;
    }
    // The claim that makes the test above worth having. Were a journal still being written, a
    // resume could be reading that file and balthasar could be doing nothing — and the two
    // arrangements would be indistinguishable from the outside.
    let dir = workspace("nj");
    let mind = Mind::answering("resume-nojournal", "noted");
    let run = magi(&dir, &mind, &["--sessions", "sessions", "-p", "hello"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let left = journals(&dir);
    assert!(
        left.is_empty(),
        "balthasar is the store; a second copy is one that goes stale: {left:?}"
    );
}

#[test]
fn resuming_where_nothing_was_kept_starts_a_session_rather_than_failing() {
    if !installed() {
        eprintln!("skipping: no balthasar on PATH");
        return;
    }
    // `--resume` in a directory nothing has run in. There is a balthasar, it answers, and it has
    // nothing to replay -- which is an ordinary first session and not an error. Worth pinning,
    // because "resume found nothing" and "resume could not ask" arrive at the same empty list.
    let dir = workspace("nk");
    let mind = Mind::answering("resume-empty", "hello there");
    let run = magi(
        &dir,
        &mind,
        &["--sessions", "sessions", "--resume", "-p", "first words"],
    );
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let asks = mind.asks();
    assert_eq!(asks.len(), 1, "it asked once: {asks:?}");
    assert!(
        asks[0].contains("first words"),
        "and asked what it was given: {}",
        asks[0]
    );
}

#[test]
fn a_finished_run_leaves_no_socket_behind() {
    if !installed() {
        eprintln!("skipping: no balthasar on PATH");
        return;
    }
    // The file as well as the process. Leaving it would not break the next magi — it sweeps —
    // but "nothing outlives the window" should be true of both, and a directory filling with
    // the names of finished sessions is how the last daemon pile announced itself.
    //
    // Here rather than against a stand-in, because a stand-in binds nothing: asserting that no
    // socket is left when none was ever made is a test that cannot fail.
    let dir = workspace("ns");
    let mind = Mind::answering("resume-nosocket", "bye");
    let run = magi(&dir, &mind, &["--sessions", "sessions", "-p", "hello"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let left: Vec<_> = std::fs::read_dir(dir.join("run/balthasar"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("api@"))
        .collect();
    assert!(left.is_empty(), "a socket outlived its session: {left:?}");
}
