//! What magi leaves running, and what it does not.
//!
//! "It dies with its magi" is enforced twice, and this covers magi's half of both. The ordinary
//! half is [`crate::balthasar::stop`] ending the child on the way out. The other half is the one
//! that matters, because the exits that leave a memory layer running are the ones with no way
//! out — a panic, a `kill -9`, an OOM — where nothing in magi runs at all. For those, magi's
//! whole contribution is asking the kernel, at spawn, to do it instead.
//!
//! So what is testable here is that magi *asks*, and that a sibling which honours the asking is
//! gone afterwards. That the real balthasar honours it is balthasar's own guarantee, and is
//! tested in balthasar's repository against its own binary — the two cannot be checked in one
//! place without one repository depending on the other.
//!
//! The stand-in is a script, for the reason `Mind` is one: what is under test is the spawn — the
//! argv magi builds and the process that results. A mock in this process would agree with magi
//! about the argv and prove nothing about the process.

use magi_model::scratch::Scratch;

use magi_testkit::Mind;
use magi_testkit::mind::MODEL;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// How long to wait for a process to go, or to be sure it has not.
const WITHIN: Duration = Duration::from_secs(10);

/// A workspace with a fake balthasar that records how it was started.
///
/// It never binds, so magi waits out its own patience and carries on without memory — which is
/// the ordinary "balthasar is not installed" path and is not what is being tested. What the
/// script leaves behind is its argv and, while it lives, a pid.
fn workspace(name: &str, tie: Tie) -> Scratch {
    let dir = Scratch::new("ml", name);
    for under in ["run", "sessions", "bin"] {
        std::fs::create_dir_all(dir.join(under)).expect("mkdir");
    }
    fake_balthasar(&dir, tie);
    install_config(&dir.join("config/magi"));
    let init = dir.join("config/magi/init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str(&format!("\nmagi.model = \"{MODEL}\"\n"));
    std::fs::write(&init, source).expect("write init");
    dir
}

/// Whether the stand-in acts on being tied, once it has been.
#[derive(Clone, Copy)]
enum Tie {
    /// Watch the caller named by `--tied` and leave when it does.
    Honoured,
    /// Ignore it and stay up, the way a balthasar that predates the flag would.
    Ignored,
}

/// A `balthasar` on the run's own `PATH` that records its argv and then waits.
///
/// The watch is written in shell rather than borrowed from the real binary because it has to be
/// conditional: a stand-in that died with its caller whatever it was told would let this suite
/// keep passing after magi stopped asking, which is the one regression these tests exist for.
///
/// Only `serve` waits, and only `serve` is recorded. magi asks a sibling what settings it takes
/// before it starts one, so a stand-in that answered every subcommand the same way recorded
/// `needs --json` as the argv under test and then slept for ten minutes holding up the run that
/// was waiting to read it.
fn fake_balthasar(dir: &Path, tie: Tie) {
    let bin = dir.join("bin");
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" != serve ]; then\n\
           # Every other subcommand: nothing to say, said quickly. A sibling that takes no\n\
           # settings is an ordinary sibling, and magi carries on without configuring it.\n\
           echo '[]'\n\
           exit 0\n\
         fi\n\
         echo \"$@\" > {argv}\n\
         echo $$ > {pid}\n\
         # `--tied <pid>` names the process to watch. Gone means gone: a pid that cannot be\n\
         # signalled is not one that is coming back.\n\
         caller=\"\"\n\
         for word in \"$@\"; do\n\
           if [ \"$prev\" = \"--tied\" ]; then caller=$word; fi\n\
           prev=$word\n\
         done\n\
         {honour}\n\
         sleep 600\n",
        argv = dir.join("argv").display(),
        pid = dir.join("balthasar.pid").display(),
        honour = match tie {
            Tie::Honoured =>
                "if [ -n \"$caller\" ]; then\n\
                   while kill -0 \"$caller\" 2>/dev/null; do sleep 0.1; done\n\
                   exit 0\n\
                 fi",
            Tie::Ignored => ":",
        },
    );
    let path = bin.join("balthasar");
    std::fs::write(&path, script).expect("write the stand-in");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
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

/// The command a run is, before it is waited on.
fn started(dir: &Path, mind: &Mind, args: &[&str]) -> Command {
    let inherited = std::env::var("PATH").unwrap_or_default();
    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    command
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env(
            "PATH",
            format!(
                "{}:{}:{inherited}",
                mind.on_path().display(),
                dir.join("bin").display()
            ),
        )
        .args(args);
    command
}

/// What the stand-in recorded about how it was started.
fn argv_of(dir: &Path) -> String {
    waited_for(&dir.join("argv")).unwrap_or_default()
}

/// The stand-in's own pid, once it has written one.
fn balthasar_pid(dir: &Path) -> u32 {
    waited_for(&dir.join("balthasar.pid"))
        .and_then(|text| text.trim().parse().ok())
        .expect("the stand-in said which process it is")
}

/// The contents of a file the stand-in writes, once it exists.
fn waited_for(path: &Path) -> Option<String> {
    let deadline = Instant::now() + WITHIN;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            return Some(text);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Whether a process exists and is not merely a corpse waiting to be reaped.
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|stat| stat.split_whitespace().nth(2) != Some("Z"))
}

/// Wait for `pid` to go, and say whether it did.
fn gone(pid: u32) -> bool {
    let deadline = Instant::now() + WITHIN;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !alive(pid)
}

/// Leave nothing running, whatever the assertions did.
fn end(pid: u32) {
    let _ = Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status();
}

#[test]
fn balthasar_is_told_which_process_to_die_with() {
    // The asking, on its own. Everything else here depends on magi passing this, so when the
    // rest of the file fails together this is the one that says why.
    let dir = workspace("asks", Tie::Ignored);
    let mind = Mind::answering("life-asks", "bye");
    let run = started(&dir, &mind, &["--sessions", "sessions", "-p", "hello"])
        .spawn()
        .expect("run magi");
    let recorded = argv_of(&dir);
    let pid = balthasar_pid(&dir);
    let mut run = run;
    let _ = run.wait();
    end(pid);

    assert!(
        recorded.contains("--tied"),
        "magi must ask the kernel to take balthasar with it: {recorded}"
    );
    let named = recorded
        .split_whitespace()
        .skip_while(|word| *word != "--tied")
        .nth(1)
        .and_then(|word| word.parse::<u32>().ok());
    assert!(
        named.is_some(),
        "`--tied` names the process to watch, and a bare flag cannot: {recorded}"
    );
}

#[test]
fn the_process_named_is_the_magi_that_started_it() {
    // A pid that is not the caller's is the same bug as no pid at all, and reads as working:
    // the sibling watches something, that something outlives it, and nothing ever fires.
    let dir = workspace("named", Tie::Ignored);
    let mind = Mind::answering("life-names", "bye");
    let mut run = started(&dir, &mind, &["--sessions", "sessions", "-p", "hello"])
        .spawn()
        .expect("run magi");
    let ours = run.id();
    let recorded = argv_of(&dir);
    let pid = balthasar_pid(&dir);
    let _ = run.wait();
    end(pid);

    let named: Option<u32> = recorded
        .split_whitespace()
        .skip_while(|word| *word != "--tied")
        .nth(1)
        .and_then(|word| word.parse().ok());
    assert_eq!(
        named,
        Some(ours),
        "magi names itself, not some other process: {recorded}"
    );
}

#[test]
fn a_hard_killed_magi_leaves_no_balthasar() {
    // The guarantee, and the one that was broken: `kill -9` runs nothing inside magi, so the
    // kill-on-the-way-out never happened and the memory layer served on with nobody to answer.
    // Sweeping was thought to cover this and never could — it clears a socket *name*, and the
    // orphan holding that name is a live process which answers, so the sweep correctly keeps it.
    let dir = workspace("killed", Tie::Honoured);
    let mind = Mind::answering("life-killed", "bye");
    let mut run = started(&dir, &mind, &["--sessions", "sessions", "-p", "hello"])
        .spawn()
        .expect("run magi");
    let pid = balthasar_pid(&dir);
    assert!(alive(pid), "the stand-in started");

    let _ = run.kill();
    let _ = run.wait();

    let went = gone(pid);
    end(pid);
    assert!(
        went,
        "a balthasar must not outlive the magi that started it, however that magi ended"
    );
}

#[test]
fn a_clean_exit_ends_it_too() {
    // The other half, which was never broken and is the one a change here would break: magi
    // ending its own child on the way out. Tested against a stand-in that ignores the tie, so
    // what is proved is magi's kill rather than the kernel's signal.
    let dir = workspace("clean", Tie::Ignored);
    let mind = Mind::answering("life-clean", "bye");
    let mut run = started(&dir, &mind, &["--sessions", "sessions", "-p", "hello"])
        .spawn()
        .expect("run magi");
    let pid = balthasar_pid(&dir);
    let status = run.wait().expect("wait");
    assert!(status.success(), "the run itself succeeded");

    let went = gone(pid);
    end(pid);
    assert!(went, "a magi that returns has already ended its balthasar");
}
