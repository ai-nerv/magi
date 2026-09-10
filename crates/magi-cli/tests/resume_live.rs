//! `--resume` when balthasar is the store, against a balthasar that is really there: there is no
//! journal at all, so the transcript comes back over a socket. It cannot be faked — the claim is
//! that a separate program still has the conversation after the process that had it is gone.

use magi_model::scratch::Scratch;

use magi_testkit::Mind;
use magi_testkit::mind::MODEL;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether there is a balthasar to test against, looked for the way magi looks for it.
fn installed() -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .any(|dir| Path::new(dir).join("balthasar").exists())
}

/// A workspace with no fake balthasar in front of the real one. A checkout, because balthasar scopes
/// a directory that is not one by walking up for a `.git`, and a stray one high above collects every
/// directory beneath it. Short names, because a unix socket path may not exceed `SUN_LEN`.
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    magi_testkit::only_its_own_store(&mut command);
    command
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

/// Fail unless the balthasar this run talked to was the one this run started. Everything below
/// asserts about a transcript coming back out of a store, and when it does not the cause is almost
/// always the machine rather than magi. Two things say so: a journal on disk, which magi keeps only
/// when balthasar is not the store; and no `balthasar/` under the run's own runtime directory, which
/// means the run recorded into whichever balthasar the developer's shell names.
fn kept_by_its_own_balthasar(dir: &Path) {
    let left = journals(dir);
    assert!(
        left.is_empty(),
        "the environment is dirty, not the code: this run fell back to a journal on disk, so \
         balthasar never held the transcript and nothing below is a statement about resume. \
         journals: {left:?}. {}",
        lying_around()
    );
    let own = dir.join("run/balthasar");
    assert!(
        own.is_dir(),
        "the environment is dirty, not the code: this run convened no balthasar of its own, \
         which is what `MAGI_API_SOCKET` in the shell that started the suite does — the run \
         then records into that session's store rather than into {}. {}",
        own.display(),
        lying_around()
    );
}

/// What the family has left lying about in the shared runtime directories, read only when something
/// has already failed. Context for a failure and never a verdict, which is why nothing asserts on it.
fn lying_around() -> String {
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|it| !it.is_empty()) else {
        return "there is no XDG_RUNTIME_DIR to sweep".to_owned();
    };
    let runtime = PathBuf::from(runtime);
    let counted = |program: &str| {
        std::fs::read_dir(runtime.join(program))
            .into_iter()
            .flatten()
            .count()
    };
    format!(
        "for context, {} holds {} melchior and {} balthasar entries; a few hundred of those \
         means every sibling probe in the suite dials a queue of corpses first, and sweeping \
         them has fixed this before",
        runtime.display(),
        counted("melchior"),
        counted("balthasar"),
    )
}

#[test]
fn a_second_run_picks_up_the_conversation_balthasar_kept() {
    if !installed() {
        eprintln!("skipping: no balthasar on PATH");
        return;
    }
    let dir = workspace("kept");
    let mind = Mind::answering("resume-kept", "noted");

    let first = magi(&dir, &mind, &["-p", "remember gerbil"]);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    // Before anything is asserted about what came back, that there was somewhere for it to come from.
    kept_by_its_own_balthasar(&dir);

    // A separate process, after the first is entirely gone along with the balthasar it started.
    let second = magi(&dir, &mind, &["--resume", "-p", "and now?"]);
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // A resume that found nothing still answers, and answers plausibly, so only the ask says anything.
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
    // Were a journal still being written, a resume could be reading that file and balthasar doing
    // nothing, and the two arrangements would be indistinguishable from the outside.
    let dir = workspace("nj");
    let mind = Mind::answering("resume-nojournal", "noted");
    let run = magi(&dir, &mind, &["-p", "hello"]);
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
    // An ordinary first session, not an error. Worth pinning, because "resume found nothing" and
    // "resume could not ask" arrive at the same empty list.
    let dir = workspace("nk");
    let mind = Mind::answering("resume-empty", "hello there");
    let run = magi(&dir, &mind, &["--resume", "-p", "first words"]);
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
    // The file as well as the process. Here rather than against a stand-in, because a stand-in binds
    // nothing: asserting no socket is left when none was made is a test that cannot fail.
    let dir = workspace("ns");
    let mind = Mind::answering("resume-nosocket", "bye");
    let run = magi(&dir, &mind, &["-p", "hello"]);
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

#[test]
fn the_memory_verbs_reach_the_model_when_balthasar_is_there() {
    // balthasar publishes `remember`, `recall` and `forget`; `config/tools.lua` declares them from
    // whatever its `verbs` returns, reading the library out of `magi.clients.balthasar`. Nothing
    // ever put one there — the catalog was searched for a name to replace and no build ships a
    // `clients/balthasar.lua` — so the served library was fetched and dropped, the block registered
    // nothing, and a session recording into balthasar offered the model no way to ask it anything.
    //
    // Asserted against what actually reached the model, because that is the only place it shows.
    if !installed() {
        eprintln!("skipping: no balthasar on PATH");
        return;
    }
    let dir = workspace("verbs");
    let mind = Mind::answering("memory-verbs", "noted");

    let run = magi(&dir, &mind, &["-p", "hello"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    kept_by_its_own_balthasar(&dir);

    let asked = mind.asks();
    let first = asked.first().expect("the model was asked something");
    for verb in ["recall", "remember", "forget"] {
        assert!(
            first.contains(&format!("\"name\":\"{verb}\"")),
            "`{verb}` never reached the model; it offers: {}",
            first
                .match_indices("\"name\":\"")
                .map(|(at, _)| first[at + 8..].split('"').next().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}
