//! `magi --headless` started by hand, against the real melchior and the real balthasar.
//!
//! **The claim is that a session with no terminal is still one you can walk up to.** Everything
//! else headless does — bind, record, take a prompt, park — was already proved by `forking_live`,
//! because a forked child is the same front end. What was never proved is the half the whole
//! feature rests on: that such a session publishes the socket a screen attaches over, so
//! `alt+,` and `alt+.` can move somebody's terminal onto it later.
//!
//! That property breaks *silently*. Nothing errors, nothing is logged, and the agent comes up
//! answering to a name on every peer's roster — with nowhere to look. The plausible change that
//! would do it is one line and reads as tidying: a session with no screen of its own does not
//! need to tell the layer where its screen is. It does.
//!
//! # Why melchior's own directory is read here, when `forking_live` refuses to
//!
//! Because the note *is* the claim. Kinship and runs have verbs — `crew`, `whoami` — that exist
//! to answer them, so reading the notes behind those would be magi holding a second opinion about
//! a layout it does not own. A screen has no verb. melchior publishes it on the pipe to the
//! harness and nowhere else, and the only other reader is a magi drawing a footer, which needs a
//! terminal this has not got. So the file is read, and then the thing it names is *dialled* —
//! which is what makes this a test of the socket rather than of the filename.
//!
//! Skipped when either sibling is missing, the way every other `*_live` test here is.

use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for something a spawned session does on its own clock.
///
/// Generous, for the reason `forking_live` gives: convening balthasar is allowed twenty seconds
/// by itself, and a loaded machine running the rest of the suite alongside is the case this must
/// not fail on.
const PATIENCE: Duration = Duration::from_secs(40);

/// How long to wait for a session's first line before calling it stuck rather than slow. A
/// backstop against a suite that never returns, not a claim about how fast a session comes up.
const HANGING: Duration = Duration::from_secs(180);

/// Held for the whole of each test here, so only one of them is running sessions at a time.
static ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`ALONE`], ignoring a poisoning left by some other test's failure.
fn alone() -> std::sync::MutexGuard<'static, ()> {
    ALONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether a sibling is on `PATH`, the way magi looks for one.
fn installed(program: &str) -> bool {
    std::env::var("PATH").is_ok_and(|path| {
        path.split(':')
            .any(|dir| Path::new(dir).join(program).exists())
    })
}

/// Whether the melchior on `PATH` is new enough to write a role down.
///
/// **Asked, not assumed.** These tests run against whatever is installed rather than against the
/// checkout beside them, and an installed melchior is as old as the last `make install` — the one
/// on this machine predates roles entirely. Without this the role assertion fails with
/// `p/main/…`, which reads exactly like magi having dropped the role on the floor and is
/// nothing of the kind. Skipping is what the other live suites here do for the same reason: a
/// stale install is not a broken magi, and reporting it as one sends the next person after a bug
/// that is not there.
fn names_a_role() -> bool {
    std::process::Command::new("melchior")
        .arg("verbs")
        .output()
        .is_ok_and(|said| String::from_utf8_lossy(&said.stdout).contains("\"role\""))
}

/// A project to run sessions in, with runtime and config trees of its own.
///
/// **A checkout**, because balthasar scopes a directory that is not one by walking up for a
/// `.git`. **Short names**, because a unix socket path may not exceed `SUN_LEN` and the whole of
/// this tree ends up inside one.
fn workspace(name: &str) -> Option<Scratch> {
    if !installed("melchior") || !installed("balthasar") {
        eprintln!("skipping: melchior and balthasar are not both on PATH");
        return None;
    }
    if !names_a_role() {
        eprintln!("skipping: the melchior on PATH is older than roles — `oslo make install`");
        return None;
    }
    let dir = Scratch::new("mh", name).settling();
    for under in ["p", "r", "c", "d"] {
        std::fs::create_dir_all(dir.join(under)).expect("mkdir");
    }
    std::fs::create_dir_all(dir.join("p/.git")).expect("mkdir");
    Some(dir)
}

/// A headless magi somebody started by hand, and the name it printed.
struct Headless {
    process: Child,
    named: String,
}

impl Drop for Headless {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Headless {
    /// Start one **with no terminal anywhere on it**, and wait until it says what it is called.
    ///
    /// All three streams are redirected, which is the arrangement being tested: this comes up
    /// from a script or a tool, not from somebody's shell. The name arriving on stdout is also
    /// how this waits — it is printed after the socket is bound, melchior has answered and
    /// balthasar has been convened, so a name means the session is up.
    ///
    /// No `--tied`. That is the difference from `forking_live`: this session is a root, with
    /// nothing above it to outlive.
    fn start(dir: &Path, role: &str, prompt: &str) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
        // Or it records into the store of whichever session the suite was started from. See
        // [`magi_testkit::only_its_own_store`].
        magi_testkit::only_its_own_store(&mut command);
        let mut process = command
            .current_dir(dir.join("p"))
            .env("XDG_RUNTIME_DIR", dir.join("r"))
            .env("XDG_CONFIG_HOME", dir.join("c"))
            .env("XDG_DATA_HOME", dir.join("d"))
            .args(["--headless", "--role", role, "--role-description"])
            .arg("reads diffs")
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("magi runs");
        // Bounded, for the reason `forking_live` gives: an unbounded read of a child's stdout
        // turns a session that never announces itself into a suite that never returns. See
        // [`magi_testkit::first_line_within`].
        let named = magi_testkit::first_line_within(&mut process, HANGING);
        let mut session = Self {
            process,
            named: String::new(),
        };
        let named = named.expect("the session never said what it was called");
        session.named = named.trim().to_owned();
        session
    }

    /// Its id: the third part of `project/role/id`, which is what a sibling addresses.
    fn id(&self) -> &str {
        self.named.rsplit('/').next().unwrap_or_default()
    }
}

/// Where melchior leaves the note saying where an agent draws.
///
/// melchior's layout, spelled here and nowhere else in magi — see the module note for why this
/// one file is allowed to know it.
fn screen_note(dir: &Path, id: &str) -> PathBuf {
    dir.join("r/melchior/p").join(format!("{id}.ui"))
}

/// Wait until `look` answers something, or give up.
fn until<T>(what: &str, mut look: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if let Some(found) = look() {
            return found;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("gave up waiting for {what}");
}

/// Whether a process is still there.
fn running(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

/// Everything this process started: its melchior and its balthasar, by their argv.
///
/// Found through `/proc` rather than asked of anybody, because the question is about the
/// *processes*: a sibling that had left the directory and gone on running is exactly the failure
/// worth catching, and it would answer nothing.
fn siblings(parent: u32) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // Field 4, taken from after the last `)`: the field before it is the command name, and a
        // command name may contain spaces and brackets.
        let theirs = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|ppid| ppid.parse::<u32>().ok());
        if theirs != Some(parent) {
            continue;
        }
        let said = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let program = String::from_utf8_lossy(&said)
            .split('\0')
            .next()
            .unwrap_or_default()
            .to_owned();
        if let Some(name) = Path::new(&program).file_name() {
            found.push((pid, name.to_string_lossy().into_owned()));
        }
    }
    found
}

/// **The one the whole feature rests on.**
///
/// A headless magi that published no screen would be up, named, recorded, on every peer's roster
/// and impossible to look at — and nothing anywhere would say so, because the agent has no
/// terminal to say it on. So the note is found, and the socket it names is dialled and attached
/// to exactly as `alt+.` attaches: `draws: true`, from the start of the transcript, expecting the
/// snapshot a screen opens with.
/// A plain test with a runtime inside it, rather than `#[tokio::test]`: the guard that keeps
/// these two from running sessions at once is a `std` lock, and holding one across an `await` is
/// a thing clippy refuses outright.
#[test]
fn a_headless_magi_publishes_the_screen_a_peer_attaches_to() {
    let _alone = alone();
    let Some(dir) = workspace("ui") else {
        return;
    };
    let agent = Headless::start(&dir, "reviewer", "remember the gerbil");

    // The role reached the directory, which is a fact a person could not otherwise hand a
    // session: nobody minted this one, so there was no `MAGI_MELCHIOR_ROLE` to inherit and
    // melchior would have called it `main`.
    assert!(
        agent.named.contains("/reviewer/"),
        "the role never reached the layer: {}",
        agent.named
    );

    let note = screen_note(&dir, agent.id());
    let screen = until("the screen to be published", || {
        std::fs::read_to_string(&note)
            .ok()
            .map(|said| PathBuf::from(said.trim().to_owned()))
            .filter(|screen| !screen.as_os_str().is_empty())
    });

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(attaches_to(&screen));
}

/// Attach to `screen` the way `alt+.` does, and insist on the transcript it opens with.
///
/// Dialled, not merely present. A note naming a path nothing answers is the same failure wearing
/// a file, and it is the one a test that only stat'ed the note would pass against.
async fn attaches_to(screen: &Path) {
    let stream = magi_ipc::connect(screen)
        .await
        .expect("the screen a peer would attach to answers");
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor(0),
            draws: true,
        })
        .await
        .expect("attach");
    let opened = tokio::time::timeout(Duration::from_secs(10), reader.read::<HarnessEvent>())
        .await
        .expect("the session answered a screen")
        .expect("a frame");
    assert!(
        matches!(opened, HarnessEvent::SessionSnapshot { .. }),
        "a screen that moved here got {opened:?} instead of a transcript"
    );
}

/// And the siblings still die with it, root or not.
///
/// `kill -9` on purpose. The exits with a way out are the easy half; the one that matters is the
/// one where nothing of magi's ever runs again, because that is what a leaked melchior or a
/// leaked balthasar would survive — and a headless agent is the kind nobody is watching when it
/// happens.
#[test]
fn a_headless_magi_takes_its_siblings_with_it_when_it_is_killed() {
    let _alone = alone();
    let Some(dir) = workspace("kil") else {
        return;
    };
    let mut agent = Headless::start(&dir, "worker", "remember the gerbil");
    let pid = agent.process.id();

    let convened = until("the siblings to be convened", || {
        let found = siblings(pid);
        let named: Vec<&str> = found.iter().map(|(_, name)| name.as_str()).collect();
        (named.contains(&"melchior") && named.contains(&"balthasar")).then_some(found)
    });

    Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("kill runs");
    let _ = agent.process.wait();
    until("the session to go", || (!running(pid)).then_some(()));
    for (theirs, name) in convened {
        until(&format!("{name} to go with it"), || {
            (!running(theirs)).then_some(())
        });
    }
}
