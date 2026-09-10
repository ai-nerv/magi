//! The `memory` role, filled by something that is not balthasar.
//!
//! `examples/remembrance` answers the family floor and the memory core from `ROLES.md` and nothing
//! else. This points `magi.memory` at it and runs two real sessions against it: the first records,
//! the second resumes. Nothing here is faked — a separate program, written against the document
//! rather than against magi, holds the conversation.

use magi_model::scratch::Scratch;
use magi_testkit::Mind;
use magi_testkit::mind::MODEL;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The shim, compiled where the test can put it on `PATH`. `None` when there is no `rustc` to
/// compile it with, which is a machine this cannot run on rather than a failure.
fn shim(into: &Path) -> Option<PathBuf> {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/remembrance/remembrance.rs");
    let dir = into.join("bin");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let out = dir.join("remembrance");
    let built = Command::new("rustc")
        .arg(&source)
        .arg("-O")
        .arg("-o")
        .arg(&out)
        .output()
        .ok()?;
    assert!(
        built.status.success(),
        "the shim does not compile: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    Some(dir)
}

/// A workspace whose configuration names the shim as its memory layer.
fn workspace(name: &str) -> Scratch {
    let dir = Scratch::new("ms", name);
    for under in ["run", "sessions", "data", ".git"] {
        std::fs::create_dir_all(dir.join(under)).expect("mkdir");
    }
    install_config(&dir.join("config/magi"));
    let init = dir.join("config/magi/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str(&format!(
        "\nmagi.model = \"{MODEL}\"\nmagi.project = \"p\"\nmagi.memory = \"remembrance\"\n"
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

/// Run the binary in `dir`, with the fake melchior and the shim in front of a real `PATH`.
fn magi(dir: &Path, mind: &Mind, bin: &Path, args: &[&str]) -> std::process::Output {
    let inherited = std::env::var("PATH").unwrap_or_default();
    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    magi_testkit::only_its_own_store(&mut command);
    command
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env(
            "PATH",
            format!("{}:{}:{inherited}", mind.on_path().display(), bin.display()),
        )
        .args(args)
        .output()
        .expect("run magi")
}

/// Fail unless the shim is what held the conversation. Everything here asserts about a transcript
/// coming back out of a store, and a magi that convened balthasar instead reads the same.
fn kept_by_the_shim(dir: &Path) -> Vec<PathBuf> {
    let kept: Vec<PathBuf> = std::fs::read_dir(dir.join(".remembrance"))
        .expect("the shim kept nothing: this session's memory was not the shim")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert!(!kept.is_empty(), "the shim's store is empty");
    kept
}

#[test]
fn a_session_runs_against_a_memory_layer_that_is_not_balthasar() {
    let dir = workspace("run");
    let Some(bin) = shim(&dir) else {
        eprintln!("skipping: no rustc to build the shim with");
        return;
    };
    let mind = Mind::answering("swap-run", "noted");

    let run = magi(&dir, &mind, &bin, &["-p", "remember gerbil"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    // Recorded by the shim and by nothing else: a journal on disk would mean magi fell back, and
    // there is no fallback to fall back to.
    let kept = kept_by_the_shim(&dir);
    assert_eq!(kept.len(), 1, "one session's turns: {kept:?}");
    let held = std::fs::read_to_string(&kept[0]).expect("read the store");
    assert!(held.contains("remember gerbil"), "{held}");
    assert!(held.contains("noted"), "{held}");
}

#[test]
fn a_second_run_picks_up_the_conversation_the_shim_kept() {
    let dir = workspace("res");
    let Some(bin) = shim(&dir) else {
        eprintln!("skipping: no rustc to build the shim with");
        return;
    };
    let mind = Mind::answering("swap-resume", "noted");

    let first = magi(&dir, &mind, &bin, &["-p", "remember gerbil"]);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    // A separate process, after the first is gone along with the shim it started.
    let second = magi(&dir, &mind, &bin, &["--resume", "-p", "and now?"]);
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // Before anything is asserted about what came back, that the shim is where it came from: a
    // magi that spawned balthasar instead would resume from balthasar and read exactly the same.
    kept_by_the_shim(&dir);

    let asks = mind.asks();
    assert_eq!(asks.len(), 2, "one ask per run: {asks:?}");
    assert!(
        !asks[0].contains("and now?"),
        "the first run knew nothing of the second"
    );
    assert!(
        asks[1].contains("remember gerbil"),
        "the earlier prompt came back out of the shim: {}",
        asks[1]
    );
    assert!(
        asks[1].contains("noted"),
        "and so did the answer to it: {}",
        asks[1]
    );
}

#[test]
fn a_memory_layer_that_lends_no_library_declares_no_memory_tools() {
    // The four model-facing verbs are extensions, declared from the client library the role's
    // program serves. The shim serves none, so the model is offered none — and, more to the point,
    // the VM must look the library up under the name the role gave it. Reading `magi.clients`
    // under `balthasar` here would find whatever balthasar this machine has installed and hand the
    // model tools aimed at a program this session never convened.
    let dir = workspace("tls");
    let Some(bin) = shim(&dir) else {
        eprintln!("skipping: no rustc to build the shim with");
        return;
    };
    let mind = Mind::answering("swap-tools", "noted");

    let run = magi(&dir, &mind, &bin, &["-p", "hello"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let ask = mind.heard();
    for tool in [
        "\"name\":\"recall\"",
        "\"name\":\"remember\"",
        "\"name\":\"why\"",
    ] {
        assert!(
            !ask.contains(tool),
            "the model was offered {tool} by a memory layer that lends no client: {ask}"
        );
    }
}

#[test]
fn the_shim_fills_the_role_by_the_gate_that_says_so() {
    let dir = Scratch::new("ms", "gate");
    let Some(bin) = shim(&dir) else {
        eprintln!("skipping: no rustc to build the shim with");
        return;
    };
    let gate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/gate-role.sh");
    let out = Command::new("sh")
        .arg(&gate)
        .arg("memory")
        .arg(bin.join("remembrance"))
        .output()
        .expect("run the gate");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
