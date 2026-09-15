//! `magi fork` against the real melchior and the real balthasar. The claim is that a separate
//! process, named by a separate program, comes up in the same run as its parent and files its memory
//! in a directory of its own, so neither sibling can be faked. Skipped when either is missing, and
//! when what is installed is too old to be asked. melchior's notes beside a socket are its own file
//! format and are not read here; `crew` and `whoami` are asked instead. The store is magi's own.

use magi_model::scratch::Scratch;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for something a spawned session does on its own clock.
const PATIENCE: Duration = Duration::from_secs(40);

/// How long to wait for a session's first line before calling it stuck: far above [`PATIENCE`], and
/// the difference between a suite that fails and a suite that never returns.
const HANGING: Duration = Duration::from_secs(180);

/// Held for the whole of each test here, so only one is running sessions at a time. Each stands up
/// two or three, and a session is a magi, a melchior and a balthasar with a store apiece.
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

/// A workspace to run in, or nothing when this machine cannot answer the question. Installed is not
/// new enough: probed rather than read off a version, because none of them carries one.
fn ready(name: &str) -> Option<Scratch> {
    if !installed("melchior") || !installed("balthasar") {
        eprintln!("skipping: melchior and balthasar are not both on PATH");
        return None;
    }
    let dir = workspace(name);
    // The refusal, not the answer: `crew` with nothing running names nobody and would skip everywhere.
    if asked(&dir, "probe", "crew", None)
        .1
        .contains("not one of agent's verbs")
    {
        eprintln!("skipping: the installed melchior predates the run-scoped `crew`");
        return None;
    }
    Some(dir)
}

/// A project to run sessions in. A checkout, because balthasar scopes a directory that is not one by
/// walking up for a `.git`; short names, because a socket path may not exceed `SUN_LEN`; settling,
/// because a balthasar tied to a magi notices it die by looking, with its sqlite still open.
fn workspace(name: &str) -> Scratch {
    let dir = Scratch::new("mf", name).settling();
    for under in ["p", "r", "c", "d"] {
        std::fs::create_dir_all(dir.join(under)).expect("mkdir");
    }
    std::fs::create_dir_all(dir.join("p/.git")).expect("mkdir");
    dir
}

/// The binary under test, in this workspace, with nothing of the developer's machine in it.
/// `MAGI_API_SOCKET` outranks the three `XDG_` variables — see [`magi_testkit::only_its_own_store`].
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

/// A session with no terminal, and the name melchior gave it. Killed on drop, including the drop an
/// `assert!` unwinds through.
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
    /// Start one and wait until it says what it is called. The name is printed once the socket is
    /// bound, melchior has answered and balthasar has been convened.
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
        // Bounded, because the alternative is not a slow test but a stuck one.
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

    /// End it the way a person would, and wait until it has finished writing. A signal rather than
    /// the `SIGKILL` in [`Drop`]: a session hands what it has to balthasar on the way out.
    fn end(&mut self) {
        let _ = Command::new("kill")
            .arg(self.process.id().to_string())
            .status();
        let _ = self.process.wait();
    }
}

/// The session forked by the process with this pid, if it is still running. Found by its argv in
/// `/proc`, because the question is about the process. The program is checked as well as the flag:
/// balthasar takes a `--tied` of its own carrying the session's pid.
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

/// Run something as `id`, with the environment a session hands the things it starts. Set here rather
/// than read off a live process, which would only check that the process agrees with itself.
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

/// Whether melchior can see `them` from `me` at all. `list` rather than `crew`: `crew` is scoped to
/// the run, so waiting on it would report every kinship failure as an empty timeout.
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

/// Every agent that has a scratch directory in this project's store, and which run it is in:
/// `<project>/balthasar/magi/<run>/<agent>/memory.db`.
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

fn running(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

#[test]
fn a_forked_child_belongs_to_the_run_that_forked_it() {
    let _alone = alone();
    let Some(dir) = ready("kin") else {
        return;
    };
    // A child that minted a run of its own would come up in a crew of one and file its memory where
    // the parent will not look, while every relation the earlier stages built stayed correct and inert.
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

    // From the other end, which is the half that would still pass if only the parent's notes were right.
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

    // `crew` heads its answer with the run it is listing, so the two answers agreeing on that line is
    // the two agents agreeing on which run they are in.
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
    // balthasar reads the agent out of the connecting peer's `/proc/<pid>/environ`, so a child
    // spawned with its parent's `BALTHASAR_AGENT` opens the parent's `memory.db`. Read off disk,
    // because asking either session which agent it is gets the right answer from both.
    let mut parent = Session::start(&dir, "remember the gerbil");
    let child = fork(&dir, &parent, "reviewer", "look at the diff");
    until("the child to be listening", || {
        listening(&dir, parent.id(), &child)
    });
    let theirs = forked_by(parent.process.id()).expect("the child is a running process");

    // The transcript reaches balthasar when a session lets go of it, so both are ended and waited
    // for. The child is stopped; the parent is signalled, the only thing that reaches a headless one.
    let (ok, said) = asked(&dir, parent.id(), "stop", Some(&child));
    assert!(ok, "the parent could not stop its own child: {said}");
    until("the child to finish", || !running(theirs));
    parent.end();

    // A balthasar that predates per-agent scratch files leaves none where there should be two, which
    // is how it is told apart from the failure being looked for: sharing an agent leaves one.
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
    // The token is minted by the parent and kept on the parent's socket, and is the whole of what
    // makes a `stop` refusable.
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
    // A child that stays up when its parent goes is a name that answers and that nobody can stop.
    // `kill -9` on purpose: the exits where nothing of the parent's ever runs again are the ones.
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
