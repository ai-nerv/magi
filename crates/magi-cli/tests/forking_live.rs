//! `magi fork` against the real melchior and the real balthasar.
//!
//! Six stages of multi-agent work went in before anything had ever *started* an agent. Kinship,
//! the run-scoped crew, the parent-and-child gate on `assign` and the `stop` token were all
//! written and all tested, and every one of them was tested against a pair of sessions somebody
//! had assembled by setting environment variables by hand. This is the first pair that was made
//! the way a person makes one.
//!
//! **None of it can be faked, and that is why it is here rather than beside the code.** The claim
//! is that a *separate process*, named by a *separate program*, comes up in the same run as its
//! parent and files its memory in a directory of its own. A stand-in for melchior would be
//! asserting that this test knows what it wants to see, and a stand-in for balthasar would leave
//! the one thing that is actually on disk — the scratch directory — with nothing in it.
//!
//! Skipped when either program is missing, the way the other `*_live` tests are — and skipped
//! when what is installed is too old to be asked, which is not the same thing. The three
//! programs are released apart and every machine will meet a build of one that predates the
//! other; a suite that failed on that would be reporting a stale install as a broken fork.
//!
//! # What is read, and what is asked
//!
//! The notes melchior leaves beside a socket say all of this — `<id>.parent`, `<id>.session` —
//! and they are deliberately not read here. They are melchior's own file format in melchior's own
//! directory, and a test in this repository that walked it would be magi holding a second opinion
//! about a layout it does not own. `crew` and `whoami` are the answers those notes exist to
//! produce, so they are what is asked. The store *is* magi's own, and it is read off disk.

use magi_model::scratch::Scratch;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for something a spawned session does on its own clock.
///
/// Generous. Convening balthasar is allowed twenty seconds by itself, and a loaded machine
/// running four of these at once is the case this must not fail on.
const PATIENCE: Duration = Duration::from_secs(40);

/// How long to wait for a session's first line before calling it stuck rather than slow.
///
/// Far above [`PATIENCE`] on purpose: this is not a claim about how quickly a session should come
/// up, it is the difference between a suite that fails and a suite that never returns.
const HANGING: Duration = Duration::from_secs(180);

/// Held for the whole of each test here, so only one of them is running sessions at a time.
///
/// **Load is the thing being kept down, not a shared fixture.** Each of these stands up two or
/// three sessions, and a session is a magi, a melchior and a balthasar — so four at once is a
/// dozen processes with a store apiece, on a machine already running the rest of the suite in
/// parallel. Measured: with them concurrent, `resume_live` began failing to reach balthasar in
/// time, which is a suite reporting the machine rather than the code.
static ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`ALONE`], ignoring a poisoning left by some other test's failure.
///
/// A panic elsewhere has already been reported; refusing to run the rest would turn one failure
/// into a page of them.
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

/// A workspace to run in, or nothing when this machine cannot answer the question.
///
/// **Installed is not the same as new enough.** These three programs are released apart, so a
/// machine is going to have a melchior that predates the run-scoped `crew` — and a test that ran
/// against one would report a stale install as a broken fork, which is the least useful failure
/// there is. Probed rather than read off a version, because none of them carries one and "does it
/// answer this" is the question anyway.
fn ready(name: &str) -> Option<Scratch> {
    if !installed("melchior") || !installed("balthasar") {
        eprintln!("skipping: melchior and balthasar are not both on PATH");
        return None;
    }
    let dir = workspace(name);
    // The refusal, not the answer. `crew` with nothing running is a perfectly good answer that
    // happens to name nobody, so a probe that looked for content would skip on every machine.
    if asked(&dir, "probe", "crew", None)
        .1
        .contains("not one of agent's verbs")
    {
        eprintln!("skipping: the installed melchior predates the run-scoped `crew`");
        return None;
    }
    Some(dir)
}

/// A project to run sessions in, with the runtime and config trees of its own.
///
/// **A checkout**, because balthasar scopes a directory that is not one by walking up for a
/// `.git` — and a stray one above the temporary directory would collect every test here into a
/// single store, where each is meant to be a run of its own.
///
/// **Short names.** A unix socket path may not exceed `SUN_LEN`, the project's name appears
/// inside the socket path, and under `gate-hermetic` the whole run is nested in a private
/// temporary directory. A descriptive name here is what pushes it over.
///
/// **Settling**, because the sessions started in here outlive the test by a moment: a balthasar
/// tied to a magi notices that process die by looking, and its sqlite is still open while it
/// does. See [`Scratch::settling`] for the leak that found.
fn workspace(name: &str) -> Scratch {
    let dir = Scratch::new("mf", name).settling();
    for under in ["p", "r", "c", "d"] {
        std::fs::create_dir_all(dir.join(under)).expect("mkdir");
    }
    std::fs::create_dir_all(dir.join("p/.git")).expect("mkdir");
    dir
}

/// The binary under test, in this workspace, with nothing of the developer's machine in it.
///
/// Three `XDG_` variables did not make that true. `MAGI_API_SOCKET` outranks all of them —
/// see [`magi_testkit::only_its_own_store`] — and the suite is developed from inside a session
/// that sets it.
fn magi(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    magi_testkit::only_its_own_store(&mut command);
    command
        .current_dir(dir.join("p"))
        .env("XDG_RUNTIME_DIR", dir.join("r"))
        .env("XDG_CONFIG_HOME", dir.join("c"))
        .env("XDG_DATA_HOME", dir.join("d"));
    command
}

/// A session with no terminal, and the name melchior gave it.
///
/// Tied to this test process, which outlives every session it starts. Killed on drop — including
/// the drop an `assert!` unwinds through, which is the case that would otherwise leave a magi, a
/// melchior and a balthasar running for every failure.
struct Session {
    process: Child,
    named: String,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Session {
    /// Start one and wait until it says what it is called.
    ///
    /// The line is how a session with no screen announces itself, and reading it is also how this
    /// waits: it is printed once the socket is bound, melchior has answered and balthasar has been
    /// convened, so a name arriving means the session is up.
    fn start(dir: &Path, prompt: &str) -> Self {
        let mut process = magi(dir)
            .arg("--tied")
            .arg(std::process::id().to_string())
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("magi runs");
        // Bounded, because the alternative is not a slow test but a stuck one. See
        // [`magi_testkit::first_line_within`]; `HANGING` is a backstop, not the patience a
        // healthy session is held to.
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

    /// End it the way a person would, and wait until it has finished writing.
    ///
    /// A signal rather than the `kill` in [`Drop`], which is `SIGKILL` and leaves the transcript
    /// where it was: a session hands what it has to balthasar on the way out, and there is no way
    /// out of a `SIGKILL`. Anything checking what reached the store has to end a session and not
    /// merely stop it existing.
    fn end(&mut self) {
        let _ = Command::new("kill")
            .arg(self.process.id().to_string())
            .status();
        let _ = self.process.wait();
    }
}

/// The session forked by the process with this pid, if it is still running.
///
/// Found by its argv, which is where `magi fork` puts the pid its child watches. Asked of `/proc`
/// rather than of melchior, because the question is about the *process*: a child that had left
/// the directory and gone on running is exactly the failure worth catching.
///
/// **The program is checked as well as the flag.** balthasar takes a `--tied` of its own and the
/// one a session convenes carries that session's pid — so a match on the flag alone finds the
/// parent's memory layer, and this reported a child that had gone and a child where there was
/// none, depending on which of the two the loop reached first.
fn forked_by(parent: u32) -> Option<u32> {
    let looking = format!("--tied\0{parent}\0");
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_string_lossy()
            .parse::<u32>()
            .ok()
            .filter(|pid| *pid != parent)
        else {
            continue;
        };
        let said = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let said = String::from_utf8_lossy(&said);
        let program = said.split('\0').next().unwrap_or_default();
        if said.contains(&looking)
            && Path::new(program)
                .file_name()
                .is_some_and(|it| it == "magi")
        {
            return Some(pid);
        }
    }
    None
}

/// Run something as `id`, with the environment a session hands the things it starts.
///
/// The four that matter: who this is, and which process the session is — the second is what a
/// fork gives its child to watch. Set here rather than taken from a running session because that
/// is exactly what `inherited` in `main.rs` does for a tool, and a test that read it back off a
/// live process would be checking that the process agrees with itself.
fn as_session(command: &mut Command, id: &str, role: &str, pid: u32) {
    command
        .env("MAGI_MELCHIOR_PROJECT", "p")
        .env("MAGI_MELCHIOR_ROLE", role)
        .env("MAGI_MELCHIOR_ID", id)
        .env("MAGI_SESSION_PID", pid.to_string());
}

/// Ask melchior something as `id`, and hand back what it said.
fn asked(dir: &Path, id: &str, verb: &str, who: Option<&str>) -> (bool, String) {
    let mut command = Command::new("melchior");
    command
        .current_dir(dir.join("p"))
        .env("XDG_RUNTIME_DIR", dir.join("r"))
        .args(["tool", "--verb", verb]);
    if let Some(who) = who {
        command.args(["--who", who]);
    }
    as_session(&mut command, id, "main", std::process::id());
    let done = command.output().expect("melchior runs");
    (
        done.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&done.stdout),
            String::from_utf8_lossy(&done.stderr)
        ),
    )
}

/// Fork a child of `parent`, and hand back the id it printed.
fn fork(dir: &Path, parent: &Session, role: &str, prompt: &str) -> String {
    let mut command = magi(dir);
    command.args(["fork", "--role", role, prompt]);
    as_session(&mut command, parent.id(), "main", parent.process.id());
    let done = command.output().expect("magi fork runs");
    assert!(
        done.status.success(),
        "the fork failed: {}",
        String::from_utf8_lossy(&done.stderr)
    );
    let id = String::from_utf8_lossy(&done.stdout).trim().to_owned();
    assert!(!id.is_empty(), "a fork that started something must name it");
    id
}

/// Whether melchior can see `them` from `me` at all.
///
/// `list` rather than `crew`, and that difference is what makes the waiting honest. `crew` is
/// scoped to the run, so a child that had minted a run of its own would never appear in one — and
/// a test that waited on `crew` before asserting on `crew` would report every kinship failure as
/// a timeout with nothing in it. `list` is the project, so it says "the child is up" without
/// having an opinion about whose it is.
fn listening(dir: &Path, me: &str, them: &str) -> bool {
    asked(dir, me, "list", None).1.contains(them)
}

/// Wait until `look` says yes, or give up.
fn until(what: &str, mut look: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if look() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("gave up waiting for {what}");
}

/// Every agent that has a scratch directory in this project's store, and which run it is in.
///
/// `<project>/balthasar/magi/<run>/<agent>/memory.db`, which is balthasar's arrangement and
/// magi's own store — so it is read rather than asked for.
fn scratches(dir: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let runs = std::fs::read_dir(dir.join("p/balthasar/magi"));
    for run in runs.into_iter().flatten().flatten() {
        let Ok(name) = run.file_name().into_string() else {
            continue;
        };
        for agent in std::fs::read_dir(run.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if agent.path().join("memory.db").is_file() {
                found.push((
                    name.clone(),
                    agent.file_name().to_string_lossy().into_owned(),
                ));
            }
        }
    }
    found.sort();
    found
}

/// Whether a process is still there.
fn running(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

#[test]
fn a_forked_child_belongs_to_the_run_that_forked_it() {
    let _alone = alone();
    let Some(dir) = ready("kin") else {
        return;
    };
    // **The claim the whole stage is about.** Every session before this one was a root, `Root` to
    // every other session on the machine, because nothing had ever handed a name and a run down.
    // A child that minted a run of its own would come up in a crew of one, file its memory where
    // the parent will not look, and read as a stranger to the thing that started it — and every
    // relation the last six stages built would go on being correct and inert.
    let parent = Session::start(&dir, "remember the gerbil");
    let child = fork(&dir, &parent, "reviewer", "look at the diff");
    until("the child to be listening", || {
        listening(&dir, parent.id(), &child)
    });

    let (ok, said) = asked(&dir, parent.id(), "crew", None);
    assert!(ok, "{said}");
    assert!(
        said.contains(parent.id()) && said.contains(&child),
        "the parent's crew is missing one of them: {said}"
    );
    assert!(
        said.contains("a subagent this session started"),
        "the child came up a stranger to its parent: {said}"
    );

    // And from the other end, which is the half that would still pass if only the parent's notes
    // were right: a child that had minted its own run would name a crew with nobody else in it.
    let (ok, said) = asked(&dir, &child, "crew", None);
    assert!(ok, "{said}");
    assert!(
        said.contains(parent.id()) && said.contains(&child),
        "the child's crew is missing one of them: {said}"
    );
    assert!(
        said.contains("the session that started this one"),
        "the child does not know whose it is: {said}"
    );

    // One run named, not two. `crew` heads its answer with the run it is listing, so the two
    // answers agreeing on that line is the two agents agreeing on which run they are in.
    let run = |said: &str| said.lines().next().unwrap_or_default().trim().to_owned();
    assert_eq!(
        run(&asked(&dir, parent.id(), "crew", None).1),
        run(&said),
        "the two agents are in different runs"
    );
}

#[test]
fn a_forked_child_files_its_scratch_beside_its_parents_and_not_in_it() {
    let _alone = alone();
    let Some(dir) = ready("scr") else {
        return;
    };
    // **The trap that costs a day.** balthasar reads the agent out of the connecting peer's
    // `/proc/<pid>/environ`, so a child spawned with its parent's `BALTHASAR_AGENT` opens the
    // parent's `memory.db` and the two file their working notes on top of one another. The
    // separation would be absent from disk while every answer went on claiming it, and nothing
    // anywhere would say so.
    //
    // Read off disk, because that is where the failure is. Asking either session which agent it
    // is would get the right answer from both, which is exactly the problem.
    let mut parent = Session::start(&dir, "remember the gerbil");
    let child = fork(&dir, &parent, "reviewer", "look at the diff");
    until("the child to be listening", || {
        listening(&dir, parent.id(), &child)
    });
    let theirs = forked_by(parent.process.id()).expect("the child is a running process");

    // The transcript reaches balthasar when a session lets go of it, so both are ended first —
    // and *waited for*, so what is read is what they finished writing rather than how far they
    // had got. The child is stopped, which is how a coordinator ends a subagent; the parent is
    // signalled, which is the only thing that reaches a process with no terminal.
    let (ok, said) = asked(&dir, parent.id(), "stop", Some(&child));
    assert!(ok, "the parent could not stop its own child: {said}");
    until("the child to finish", || !running(theirs));
    parent.end();

    // A balthasar that predates per-agent scratch files nothing under `<run>/<agent>/` for
    // *anybody*, forked or not, so it cannot be asked this question at all. Told apart from the
    // failure being looked for by which session is missing: sharing an agent leaves one directory
    // where there should be two, and an old balthasar leaves none where there should be two.
    let found = scratches(&dir);
    if found.is_empty() {
        eprintln!("skipping: the installed balthasar does not key scratch by agent");
        return;
    }
    assert_eq!(
        found.len(),
        2,
        "two sessions ran and this is what they left: {found:?}"
    );
    assert_eq!(
        found[0].0, found[1].0,
        "the two agents filed their memory under different runs: {found:?}"
    );
    let agents: Vec<&str> = found.iter().map(|(_, agent)| agent.as_str()).collect();
    assert!(
        agents.contains(&child.as_str()),
        "the child's scratch is not under its own name: {found:?}"
    );
    assert_ne!(
        agents[0], agents[1],
        "both sessions filed their scratch as the same agent: {found:?}"
    );
}

#[test]
fn a_child_is_stopped_by_the_session_that_started_it_and_by_nobody_else() {
    let _alone = alone();
    let Some(dir) = ready("stp") else {
        return;
    };
    // The token. It is minted by the parent, kept on the parent's socket, and it is the whole of
    // what makes a `stop` refusable — so it is worth checking that a session which is merely
    // *there* cannot end somebody else's subagent.
    let parent = Session::start(&dir, "remember the gerbil");
    let stranger = Session::start(&dir, "a session of my own");
    let child = fork(&dir, &parent, "reviewer", "look at the diff");
    until("the child to be listening", || {
        listening(&dir, parent.id(), &child)
    });

    let (ok, said) = asked(&dir, stranger.id(), "stop", Some(&child));
    assert!(
        !ok,
        "a session that started nothing stopped somebody: {said}"
    );
    assert!(
        listening(&dir, parent.id(), &child),
        "the child went anyway"
    );

    let (ok, said) = asked(&dir, parent.id(), "stop", Some(&child));
    assert!(ok, "the session that started it could not end it: {said}");
    until("the child to go", || !listening(&dir, parent.id(), &child));
}

#[test]
fn a_child_does_not_outlive_the_session_that_forked_it() {
    let _alone = alone();
    let Some(dir) = ready("out") else {
        return;
    };
    // A child that stays up when its parent goes is a name in the directory that answers and that
    // nobody can stop: the token a `stop` is checked against went with the session that minted it.
    // `kill -9` on purpose — the exits with a way out are the easy half, and the ones that matter
    // are the ones where nothing of the parent's ever runs again.
    let parent = Session::start(&dir, "remember the gerbil");
    let child = fork(&dir, &parent, "reviewer", "look at the diff");
    until("the child to be listening", || {
        listening(&dir, parent.id(), &child)
    });

    let pid = parent.process.id();
    let theirs = forked_by(pid).expect("the child is a process with the parent's pid on its argv");
    Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("kill runs");
    drop(parent);
    until("the parent to go", || !running(pid));
    until("the child to go with it", || !running(theirs));
    assert!(
        forked_by(pid).is_none(),
        "a child outlived the session that forked it"
    );
}
