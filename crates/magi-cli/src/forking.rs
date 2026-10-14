//! `magi fork` — starting a child session, and telling it who it is. melchior names and mints the
//! secret that makes a `stop` refusable, because it holds the directory; magi spawns, because what
//! a harness is — binary, arguments, directory — is none of the layer's business. Everything
//! the child inherits arrives in one `environment` block melchior wrote, with two exceptions:
//!
//! - `BALTHASAR_AGENT` is set here, to the child's own id, in the child's own spawn environment.
//!   The child's `balthasar serve` is started with it and records for that agent, and a child that
//!   inherited its parent's would share its `memory.db`.
//! - `MAGI_API_SOCKET` is removed rather than passed on. Set, [`crate::balthasar::start`] answers
//!   `Started::Theirs` and convenes none, so the child would record into a store that dies with the
//!   parent — and a session that cannot record does not start.
//!
//! The child is handed the session's own pid, from [`SESSION_PID`], to watch. `PR_SET_PDEATHSIG`
//! cannot serve: it fires when the *immediate* parent goes, which for a forked child is this
//! process, which prints an id and exits a moment later. See [`crate::child`].

use anyhow::{Context, Result, bail};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a session tells everything it starts about the process it is — magi's own name, read here
/// and nowhere else.
pub const SESSION_PID: &str = "MAGI_SESSION_PID";

/// How long to watch a freshly spawned child before calling it started. Short, and not waiting for
/// it to be working: long enough to catch the failures that are instant, which are the ones the
/// person who typed `magi fork` is the only party left who could be told about.
const WATCH: Duration = Duration::from_millis(750);

/// What `melchior fork` prints: a name, the child's stop secret, and the environment to start it
/// with. The secret is never put in that environment, whichever field it arrives in.
#[derive(Debug, serde::Deserialize)]
struct Minted {
    id: String,
    #[serde(default)]
    token: Option<String>,
    environment: std::collections::BTreeMap<String, String>,
}

/// Name a child, start it, and say what it is called. `role` and `role_description` go to melchior
/// rather than onto the spawn, so a role is in the directory at birth. `prompt` is optional.
pub fn fork(
    role: Option<&str>,
    role_description: Option<&str>,
    prompt: Option<&str>,
) -> Result<()> {
    let parent = session_pid()?;
    if !has_leave(parent) {
        bail!("this session was started without leave to start children of its own");
    }
    let loaded = crate::config::load().ok();
    let program = loaded.as_ref().map_or_else(
        || magi_host::broker::MELCHIOR.to_owned(),
        crate::config::mind,
    );
    let mut minted = mint(&program, role, role_description)?;
    let secret = minted
        .token
        .take()
        .or_else(|| minted.environment.remove(crate::melchior::TOKEN_VAR));
    minted.environment.remove(crate::melchior::TOKEN_VAR);
    let handed = secret
        .as_deref()
        .map(crate::melchior::hand_token)
        .transpose()
        .context("the child's stop secret could not be put where only it can read it")?;
    let harness = std::env::current_exe()
        .context("magi could not find its own binary, so it has nothing to start a child with")?;
    let logs = magi_ipc::private_dir(&magi_ipc::runtime_dir("magi")?.join("children"))?;
    let started = start(&harness, &minted, parent, prompt, handed.as_deref(), &logs);
    if started.is_err()
        && let Some(path) = &handed
    {
        let _ = std::fs::remove_file(path);
    }
    started?;
    println!("{}", minted.id);
    Ok(())
}

/// The session this fork belongs to. Refused rather than guessed at: a fork with no parent to watch
/// is the orphan this whole arrangement exists to prevent.
fn session_pid() -> Result<u32> {
    session_pid_from(std::env::var(SESSION_PID).ok().as_deref())
}

/// The same answer, with the environment handed in rather than read.
fn session_pid_from(said: Option<&str>) -> Result<u32> {
    said.map(str::trim)
        .filter(|said| !said.is_empty())
        .and_then(|said| said.parse().ok())
        .with_context(|| {
            format!(
                "`magi fork` starts a child of *this* session, and nothing here says which \
                 process that is. It is run from inside one — from a tool, or a shell a session \
                 started — where ${SESSION_PID} says so."
            )
        })
}

/// Whether the session `pid` was started with leave to start children: `MAGI_NO_SPAWN` absent from
/// the environment the kernel recorded when it was started. Read there rather than from this
/// process, whose own `MAGI_NO_SPAWN` is what `spawn` hands a child that may not delegate. A
/// session whose environment cannot be read has no leave.
fn has_leave(pid: u32) -> bool {
    let wanted = format!("{}=", magi_tools::builtin::NO_SPAWN);
    std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
        !environ
            .split(|byte| *byte == 0)
            .any(|entry| entry.starts_with(wanted.as_bytes()))
    })
}

/// Ask this session's melchior to name a child and mint its secret over argv. `melchior fork` calls
/// its own socket, so the party holding the secret is the one that already holds the session.
fn mint(program: &str, role: Option<&str>, description: Option<&str>) -> Result<Minted> {
    let mut asking = Command::new(program);
    asking.arg("fork");
    if let Some(role) = role {
        asking.arg("--role").arg(role);
    }
    if let Some(said) = description {
        asking.arg("--role-description").arg(said);
    }
    let answered = asking.output().with_context(|| {
        format!(
            "`{program} fork` could not be run. melchior is what names a session, so without it \
             there is nothing to call a child and no secret to stop it with."
        )
    })?;
    if !answered.status.success() {
        bail!(
            "`{program} fork` would not name a child: {}",
            String::from_utf8_lossy(&answered.stderr).trim()
        );
    }
    serde_json::from_slice(&answered.stdout)
        .context("melchior named a child in a shape this magi cannot read")
}

/// Exactly how a child is started: what is on its argv, and what is in its environment. Split from
/// [`start`] so the tests read a real one back rather than a copy that outlives a changed spawn.
fn spawning(
    harness: &std::path::Path,
    minted: &Minted,
    parent: u32,
    prompt: Option<&str>,
    token: Option<&std::path::Path>,
) -> Command {
    let mut starting = Command::new(harness);
    // Both, though `--tied` implies the first: argv is what a person reads off `/proc`, and "no
    // terminal" and "dies with 4242" are two facts.
    starting
        .arg("--headless")
        .arg("--tied")
        .arg(parent.to_string());
    // After `--`, so a prompt that begins with a dash is a prompt and not a flag.
    if let Some(prompt) = prompt {
        starting.arg("--").arg(prompt);
    }
    starting.envs(&minted.environment);
    for named in crate::balthasar::AGENT {
        starting.env(named, &minted.id);
    }
    if let Some(token) = token {
        starting.env(crate::melchior::TOKEN_FILE, token);
    }
    starting
        .env_remove(crate::melchior::TOKEN_VAR)
        .env_remove("MAGI_API_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    starting
}

/// Spawn the child, and give up on it if it dies while we are still watching. stdout goes nowhere:
/// `magi fork` is usually run through a tool whose output is a pipe somebody reads to the end, and
/// a child holding it open is a tool call that never returns. stderr goes to `<logs>/<id>.log`,
/// which outlives this process, so the child can always write it. `harness` comes from the caller
/// so a test can point it at a program that does not start.
fn start(
    harness: &std::path::Path,
    minted: &Minted,
    parent: u32,
    prompt: Option<&str>,
    token: Option<&std::path::Path>,
    logs: &std::path::Path,
) -> Result<()> {
    let log = logs.join(format!("{}.log", minted.id));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&log)
        .with_context(|| format!("the child's log {} could not be opened", log.display()))?;
    let mut child = spawning(harness, minted, parent, prompt, token)
        .stderr(written)
        .spawn()
        .context("the child session could not be started")?;
    let deadline = Instant::now() + WATCH;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap_or(None) {
            bail!(
                "the child session exited ({status}) before it was up: {}",
                last_words(&log)
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

/// What a child that would not start said on its way out: the last line of its log.
fn last_words(log: &std::path::Path) -> String {
    let said = std::fs::read_to_string(log).unwrap_or_default();
    said.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("it said nothing")
        .trim()
        .to_owned()
}

/// What a child is handed, and what it must not be handed.
#[cfg(test)]
mod tests {
    use super::*;

    /// A descriptor shaped as melchior's `mint` writes one.
    fn minted() -> Minted {
        serde_json::from_str(
            r#"{"project":"magi","role":"reviewer","description":"reads diffs","id":"iota-mu",
                "full":"magi/reviewer/iota-mu","parent":"alpha-rho","token":"deadbeef",
                "session":"alpha-rho-1788913233",
                "environment":{
                  "MAGI_MELCHIOR_PROJECT":"magi","MAGI_MELCHIOR_ROLE":"reviewer\nreads diffs",
                  "MAGI_MELCHIOR_ID":"iota-mu","MAGI_MELCHIOR_PARENT":"alpha-rho",
                  "MAGI_MELCHIOR_TOKEN":"deadbeef","MAGI_MELCHIOR_SESSION":"alpha-rho-1788913233"
                }}"#,
        )
        .expect("a descriptor magi can read")
    }

    /// Everything the spawn would set, read back off the very `Command` [`start`] would run, so a
    /// copy written out here cannot go on passing after somebody changes the spawn.
    fn spawn_environment(minted: &Minted) -> std::collections::BTreeMap<String, Option<String>> {
        spawning(std::path::Path::new("magi"), minted, 4242, None, None)
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn a_child_is_told_its_own_agent_and_never_its_parents() {
        // The trap: set to the parent's agent, both sessions would open one `memory.db`.
        let minted = minted();
        let environ = spawn_environment(&minted);
        // Under both names, because which one the memory layer reads is its own business and a
        // child that named itself under only the one it does not read is nameless to it.
        for named in crate::balthasar::AGENT {
            assert_eq!(
                environ.get(named).and_then(Clone::clone),
                Some("iota-mu".to_owned()),
                "the child came up as somebody else under {named}"
            );
            assert_ne!(
                environ.get(named).and_then(Clone::clone),
                Some("alpha-rho".to_owned())
            );
        }
    }

    #[test]
    fn a_child_belongs_to_the_run_that_started_it() {
        // Not to one of its own: a child that minted a run would read as `Root` to its own parent.
        let minted = minted();
        let environ = spawn_environment(&minted);
        assert_eq!(
            environ
                .get(crate::melchior::SESSION)
                .and_then(Clone::clone)
                .as_deref(),
            Some("alpha-rho-1788913233")
        );
        assert_eq!(
            environ
                .get("MAGI_MELCHIOR_PARENT")
                .and_then(Clone::clone)
                .as_deref(),
            Some("alpha-rho")
        );
        assert_eq!(
            environ.get(crate::melchior::TOKEN_VAR),
            Some(&None),
            "the stop secret travels in a file, never in the environment"
        );
    }

    #[test]
    fn a_child_convenes_its_own_store_rather_than_its_parents() {
        // `env_remove` shows up as a name with no value, which is what unsets it. Inherited, the
        // child would keep its transcript in a process that dies with the coordinator.
        let minted = minted();
        let environ = spawn_environment(&minted);
        assert_eq!(
            environ.get("MAGI_API_SOCKET"),
            Some(&None),
            "the child would have talked to its parent's balthasar"
        );
    }

    #[test]
    fn a_role_and_what_it_is_for_travel_together_as_melchior_wrote_them() {
        // Read whole or not at all: a name from the flag and a sentence from elsewhere is no role.
        let minted = minted();
        let environ = spawn_environment(&minted);
        assert_eq!(
            environ
                .get("MAGI_MELCHIOR_ROLE")
                .and_then(Clone::clone)
                .as_deref(),
            Some("reviewer\nreads diffs")
        );
    }

    #[test]
    fn a_fork_with_no_session_to_belong_to_is_refused() {
        // A child watching a pid nobody named is the orphan this arrangement exists to prevent.
        for nothing in [None, Some(""), Some("   "), Some("not a pid")] {
            let why = session_pid_from(nothing)
                .expect_err("a fork with no parent must not start")
                .to_string();
            assert!(why.contains(SESSION_PID), "{nothing:?}: {why}");
        }
        assert_eq!(session_pid_from(Some(" 4242 ")).expect("a pid"), 4242);
    }

    #[test]
    fn a_melchior_that_is_not_installed_is_said_out_loud() {
        // The one failure a person will actually meet: it must name the program.
        let why = mint("melchior-that-is-not-installed", None, None)
            .expect_err("nothing named the child")
            .to_string();
        assert!(why.contains("melchior-that-is-not-installed"), "{why}");
    }

    #[test]
    fn a_child_that_dies_at_once_is_reported_rather_than_announced() {
        // `magi fork` prints an id, and printing one is a promise that something answers to it.
        let dir = magi_model::scratch::Scratch::new("magi-forking", "dies");
        let script = a_harness(
            &dir,
            "echo 'magi could not convene balthasar' >&2\nexit 3\n",
        );
        let why = start(&script, &minted(), std::process::id(), None, None, &dir)
            .expect_err("a child that exited must not be announced")
            .to_string();
        assert!(why.contains("could not convene balthasar"), "{why}");
    }

    #[test]
    fn what_the_child_comes_up_with_is_what_the_kernel_wrote_at_exec() {
        // The claim the whole file rests on, checked against a real child's own initial block:
        // `setenv` in this process would not appear there.
        let dir = magi_model::scratch::Scratch::new("magi-forking", "block");
        let seen = dir.join("environ");
        let script = a_harness(
            &dir,
            &format!(
                "tr '\\0' '\\n' < /proc/self/environ > {}\nsleep 2\n",
                seen.display()
            ),
        );
        start(
            &script,
            &minted(),
            std::process::id(),
            Some("get on with it"),
            None,
            &dir,
        )
        .expect("a child that stays up is started");

        let block = std::fs::read_to_string(&seen).expect("the child wrote its own environment");
        assert!(
            block.lines().any(|line| line == "BALTHASAR_AGENT=iota-mu"),
            "the child's scratch would land in its parent's directory: {block}"
        );
        assert!(
            block
                .lines()
                .any(|line| line == "MAGI_MELCHIOR_SESSION=alpha-rho-1788913233"),
            "the child started a run of its own: {block}"
        );
        assert!(
            !block
                .lines()
                .any(|line| line.starts_with("MAGI_API_SOCKET=")),
            "the child would keep its transcript in its parent's balthasar: {block}"
        );
    }

    #[test]
    fn a_child_is_told_the_pid_to_watch_and_what_to_get_on_with() {
        // Both on argv, not in the environment: a shell the child starts must inherit neither.
        let dir = magi_model::scratch::Scratch::new("magi-forking", "argv");
        let seen = dir.join("argv");
        let script = a_harness(
            &dir,
            &format!("printf '%s\\n' \"$@\" > {}\nsleep 2\n", seen.display()),
        );
        start(&script, &minted(), 4242, Some("read the diff"), None, &dir)
            .expect("a child that stays up is started");

        let said = std::fs::read_to_string(&seen).expect("the child wrote its arguments");
        assert_eq!(
            said.lines().collect::<Vec<_>>(),
            ["--headless", "--tied", "4242", "--", "read the diff"]
        );
    }

    #[test]
    fn a_child_is_told_where_its_secret_waits_and_not_the_secret() {
        let handed = std::path::Path::new("/run/user/1000/magi/tokens/0123456789abcdef.token");
        let environ: std::collections::BTreeMap<_, _> = spawning(
            std::path::Path::new("magi"),
            &minted(),
            4242,
            None,
            Some(handed),
        )
        .get_envs()
        .map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned)))
        .collect();
        assert_eq!(
            environ.get(std::ffi::OsStr::new(crate::melchior::TOKEN_FILE)),
            Some(&Some(handed.as_os_str().to_owned()))
        );
        assert!(
            environ.values().flatten().all(|value| value != "deadbeef"),
            "the secret itself is in the environment"
        );
    }

    #[test]
    fn a_prompt_that_looks_like_a_flag_is_still_a_prompt() {
        use clap::Parser;
        let argv: Vec<String> = spawning(
            std::path::Path::new("magi"),
            &minted(),
            4242,
            Some("-p rm everything"),
            None,
        )
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
        let parsed = crate::Cli::try_parse_from(std::iter::once("magi".to_owned()).chain(argv))
            .expect("parses");
        assert!(!parsed.print, "the prompt became a flag");
        assert_eq!(parsed.prompt.as_deref(), Some("-p rm everything"));
    }

    /// A process standing in for a session, started with or without leave to spawn.
    fn a_session(leave: bool) -> std::process::Child {
        let mut session = Command::new("sleep");
        session.arg("30");
        if leave {
            session.env_remove(magi_tools::builtin::NO_SPAWN);
        } else {
            session.env(magi_tools::builtin::NO_SPAWN, "1");
        }
        session.spawn().expect("a stand-in session")
    }

    #[test]
    fn a_session_without_leave_to_spawn_forks_nothing() {
        // Probed in a child process, so the variables are set in its environment and not this one's.
        if let Ok(expected) = std::env::var("MAGI_FORK_PROBE") {
            let why = fork(None, None, None)
                .expect_err("nothing to mint with")
                .to_string();
            assert_eq!(
                why.contains("without leave"),
                expected == "refused",
                "{why}"
            );
            return;
        }
        let dir = magi_model::scratch::Scratch::new("magi-fork", "leave");
        let probe = |session: &std::process::Child, own: bool, expected: &str| {
            let mut ran = Command::new(std::env::current_exe().expect("test binary"));
            ran.args([
                "--exact",
                "forking::tests::a_session_without_leave_to_spawn_forks_nothing",
            ])
            .env("MAGI_FORK_PROBE", expected)
            .env(SESSION_PID, session.id().to_string())
            .env("PATH", &*dir)
            .env("XDG_CONFIG_HOME", &*dir)
            .env_remove(magi_tools::builtin::NO_SPAWN);
            if own {
                ran.env(magi_tools::builtin::NO_SPAWN, "1");
            }
            let ran = ran.output().expect("probe");
            assert!(
                ran.status.success(),
                "{}",
                String::from_utf8_lossy(&ran.stdout)
            );
        };
        let (mut with, mut without) = (a_session(true), a_session(false));
        assert!(has_leave(with.id()) && !has_leave(without.id()));
        probe(&without, false, "refused");
        // What `spawn` hands a child that may not delegate does not stop the fork carrying it.
        probe(&with, true, "allowed");
        for session in [&mut with, &mut without] {
            let _ = session.kill();
            let _ = session.wait();
        }
    }
    /// A runnable stand-in for the magi a fork would start, written into `dir`. A script, not
    /// a mock, because what is under test is the argv and the environment block the kernel writes.
    fn a_harness(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("magi");
        let mut file = std::fs::File::create(&path).expect("write the stand-in");
        // The probe below runs it, so it must have a way out that does nothing.
        write!(file, "#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\n{body}").expect("write");
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        // `ETXTBSY`: between a `fork` on another thread and its `exec` the child holds every
        // descriptor this one had, including the file just written.
        while let Err(why) = Command::new(&path).arg("--probe").status() {
            assert_eq!(
                why.raw_os_error(),
                Some(26),
                "the stand-in will not run: {why}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        path
    }
}
