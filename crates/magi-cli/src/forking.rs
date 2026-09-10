//! `magi fork` — starting a child session, and telling it who it is. melchior names and mints the
//! secret that makes a `stop` refusable, because it holds the directory; magi spawns, because what
//! a harness is — binary, arguments, directory — is none of the layer's business. Everything
//! the child inherits arrives in one `environment` block melchior wrote, with two exceptions:
//!
//! - `BALTHASAR_AGENT` is set here, to the child's own id, in the child's own spawn environment.
//!   balthasar reads the agent out of the connecting peer's `/proc/<pid>/environ` — the block the
//!   kernel wrote at `exec` — and a child that inherited its parent's would share its `memory.db`.
//! - `MAGI_API_SOCKET` is removed rather than passed on. Set, [`crate::balthasar::start`] answers
//!   `Started::Theirs` and convenes none, so the child would record into a store that dies with the
//!   parent — and a session that cannot record does not start.
//!
//! The child is handed the session's own pid, from [`SESSION_PID`], to watch. `PR_SET_PDEATHSIG`
//! cannot serve: it fires when the *immediate* parent goes, which for a forked child is this
//! process, which prints an id and exits a moment later. See [`crate::child`].

use anyhow::{Context, Result, bail};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a session tells everything it starts about the process it is — magi's own name, read here
/// and nowhere else.
pub const SESSION_PID: &str = "MAGI_SESSION_PID";

/// How long to watch a freshly spawned child before calling it started. Short, and not waiting for
/// it to be working: long enough to catch the failures that are instant, which are the ones the
/// person who typed `magi fork` is the only party left who could be told about.
const WATCH: Duration = Duration::from_millis(750);

/// What `melchior fork` prints: a name, a secret, and the environment to start a child with. Three
/// fields out of the several it says; the rest appear inside `environment` as well.
#[derive(Debug, serde::Deserialize)]
struct Minted {
    id: String,
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
    let loaded = crate::config::load().ok();
    let program = loaded.as_ref().map_or_else(
        || magi_host::broker::MELCHIOR.to_owned(),
        crate::config::mind,
    );
    let minted = mint(&program, role, role_description)?;
    let harness = std::env::current_exe()
        .context("magi could not find its own binary, so it has nothing to start a child with")?;
    start(&harness, &minted, parent, prompt)?;
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
) -> Command {
    let mut starting = Command::new(harness);
    // Both, though `--tied` implies the first: argv is what a person reads off `/proc`, and "no
    // terminal" and "dies with 4242" are two facts.
    starting
        .arg("--headless")
        .arg("--tied")
        .arg(parent.to_string());
    if let Some(prompt) = prompt {
        starting.arg(prompt);
    }
    starting.envs(&minted.environment);
    for named in crate::balthasar::AGENT {
        starting.env(named, &minted.id);
    }
    starting
        .env_remove("MAGI_API_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    starting
}

/// Spawn the child, and give up on it if it dies while we are still watching. stdout and stderr go
/// nowhere: `magi fork` is usually run through a tool whose output is a pipe somebody reads to the
/// end: a child holding it open is a tool call that never returns. stderr is piped for [`WATCH`]
/// only. `harness` comes from the caller so a test can point it at a program that does not start.
fn start(
    harness: &std::path::Path,
    minted: &Minted,
    parent: u32,
    prompt: Option<&str>,
) -> Result<()> {
    let mut child = spawning(harness, minted, parent, prompt)
        .spawn()
        .context("the child session could not be started")?;
    let deadline = Instant::now() + WATCH;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap_or(None) {
            bail!(
                "the child session exited ({status}) before it was up: {}",
                last_words(&mut child)
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

/// What a child that would not start said on its way out: the last line only, read after it has
/// exited so the pipe is closed and this cannot block.
fn last_words(child: &mut std::process::Child) -> String {
    use std::io::Read;
    let mut said = String::new();
    if let Some(pipe) = child.stderr.as_mut() {
        let _ = pipe.read_to_string(&mut said);
    }
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
        spawning(std::path::Path::new("magi"), minted, 4242, None)
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
            environ
                .get("MAGI_MELCHIOR_TOKEN")
                .and_then(Clone::clone)
                .as_deref(),
            Some("deadbeef"),
            "without the token nothing could ever stop it"
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
        let why = start(&script, &minted(), std::process::id(), None)
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
        start(&script, &minted(), 4242, Some("read the diff"))
            .expect("a child that stays up is started");

        let said = std::fs::read_to_string(&seen).expect("the child wrote its arguments");
        assert_eq!(
            said.lines().collect::<Vec<_>>(),
            ["--headless", "--tied", "4242", "read the diff"]
        );
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
