//! `magi fork` — starting a child session, and telling it who it is.
//!
//! **melchior names, magi spawns, and neither does the other's half.** melchior holds the
//! directory, so it is the only thing that can choose a name nobody else is answering to and mint
//! the secret that makes a `stop` refusable. What a harness *is* — which binary, which arguments,
//! which working directory — is nothing the layer has any business knowing. So `melchior fork`
//! answers with a descriptor and this file starts a process with it.
//!
//! Everything the child inherits arrives in one `environment` block that melchior wrote, so
//! adding a variable to the lattice is a change in melchior alone. Two things are *not* in it,
//! and both are on purpose:
//!
//! - **`BALTHASAR_AGENT`, which melchior has never heard of.** It is set here, to the child's own
//!   id, in the child's own spawn environment. balthasar reads the agent out of the connecting
//!   peer's `/proc/<pid>/environ` — the block the kernel wrote at `exec` — so it has to be in
//!   place before the child runs, and it has to be the child's. A child that inherited its
//!   parent's would file its scratch in the parent's directory: the two would share a memory.db,
//!   and the separation the agent dimension exists to give would be absent from disk while every
//!   answer went on claiming it.
//!
//! - **`MAGI_API_SOCKET`, which is removed rather than passed on.** Set, it means "somebody else
//!   already said which balthasar to talk to", and [`crate::balthasar::start`] answers
//!   `Started::Theirs` and convenes none. The child would then keep its transcript in the
//!   *parent's* store process — which files scratch under the right agent, because balthasar pins
//!   that per connection, and which dies with the parent. A session refuses to start when it
//!   cannot record, so coupling the two lifetimes turns "the coordinator quit" into "the subagent
//!   cannot open". One process each is the cheaper of the two.
//!
//! # Why the child is given a pid to watch
//!
//! A child that outlives its parent is a name in the directory that answers and that nobody can
//! stop: the token `stop` is checked against was minted by the parent and held on the parent's
//! socket, and both went with it.
//!
//! balthasar's answer to the same problem is `PR_SET_PDEATHSIG`, and it cannot be borrowed here.
//! That signal fires when the *immediate* parent goes, and the immediate parent of a forked child
//! is this process — which prints an id and exits a moment later. So the session's own pid is
//! handed across instead, from [`SESSION_PID`], and the child watches it. See [`crate::child`],
//! which does the watching and says what it does about a pid coming round again.

use anyhow::{Context, Result, bail};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a session tells everything it starts about the process it is.
///
/// magi's own name, for magi's own children: it is a pid, so it means nothing to melchior and
/// nothing to balthasar. Read here and nowhere else. A session that set it and never spawned
/// anything would be telling nobody something true.
pub const SESSION_PID: &str = "MAGI_SESSION_PID";

/// How long to watch a freshly spawned child before calling it started.
///
/// Short, and it is not waiting for the child to be *working* — convening balthasar alone is
/// allowed twenty seconds. It is long enough to catch the failures that are instant: a magi that
/// cannot be exec'd, a configuration it refuses, a balthasar that is not installed. Those are the
/// ones where the person who typed `magi fork` is the only party left who could be told, because
/// after this the child has no terminal and says nothing to anybody.
const WATCH: Duration = Duration::from_millis(750);

/// What `melchior fork` prints: a name, a secret, and the environment to start a child with.
///
/// Three fields out of the several it says. The rest are the same facts spelled differently —
/// `parent`, `token` and `session` all appear inside `environment` as well — and reading them
/// twice would be magi holding an opinion about a lattice that is melchior's.
#[derive(Debug, serde::Deserialize)]
struct Minted {
    /// The child's id: the third part of `project/role/id`, and what a sibling addresses.
    id: String,
    /// Everything the child inherits, under melchior's own names for it.
    environment: std::collections::BTreeMap<String, String>,
}

/// Name a child, start it, and say what it is called.
///
/// `role` and `role_description` say what the child is *for*, and they go to melchior rather than
/// onto the spawn: a role is written into the directory at birth, so that there is no window in
/// which a child is up, on every peer's roster, and described as `main`.
///
/// `prompt` is what it should get on with. Optional, because a child that was forked to be
/// *given* work — by message, or by a person pressing `>` — is a reasonable thing to fork.
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

/// The session this fork belongs to, as the process it is.
///
/// Refused rather than guessed at. A fork with no parent to watch is exactly the orphan this
/// whole arrangement exists to prevent, and "started nothing" is a better answer than "started
/// something nobody can end".
fn session_pid() -> Result<u32> {
    session_pid_from(std::env::var(SESSION_PID).ok().as_deref())
}

/// The same answer, with the environment handed in rather than read.
///
/// Split out so both cases can be checked without setting a process-wide variable — the same
/// arrangement `run_from` in [`crate::melchior`] is under, and for the same reason.
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

/// Ask this session's melchior to name a child and mint its secret.
///
/// Over argv, like every other question with an answer. `melchior fork` finds this session from
/// the environment and calls its *own* socket, so the party that ends up holding the secret is
/// the party that already holds the session — which is what makes it worth anything.
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

/// Exactly how a child is started: what is on its argv, and what is in its environment.
///
/// Split from [`start`] so that the tests below can read a real one back rather than assemble a
/// second copy of the same decisions. A copy is what makes a test pass against the bug it was
/// written for: change the spawn and the copy goes on asserting the old thing, cheerfully.
fn spawning(
    harness: &std::path::Path,
    minted: &Minted,
    parent: u32,
    prompt: Option<&str>,
) -> Command {
    let mut starting = Command::new(harness);
    starting.arg("--tied").arg(parent.to_string());
    if let Some(prompt) = prompt {
        starting.arg(prompt);
    }
    starting
        .envs(&minted.environment)
        .env(crate::balthasar::AGENT, &minted.id)
        .env_remove("MAGI_API_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    starting
}

/// Spawn the child, and give up on it if it dies while we are still watching.
///
/// **stdout and stderr go nowhere**, and that is not tidiness. `magi fork` is usually run by a
/// model through a tool, whose output is a pipe somebody reads to the end — and a child holding
/// that pipe open for the rest of its life is a tool call that never returns. The child has no
/// terminal by design; what it has to say, it says on its own screen, which is what `>` is for.
///
/// stderr is piped rather than dropped only for [`WATCH`], because the one moment a child can
/// usefully complain to anybody is before it is up, and this process is the only thing listening.
///
/// `harness` is this binary, taken from the caller rather than looked up here so that a test can
/// point it at a program that comes up and one that does not.
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

/// What a child that would not start said on its way out.
///
/// The last line rather than all of them: this goes into a sentence somebody reads, and a stack
/// of them buries the one that names the cause. Read only after the child has exited, so the pipe
/// is closed and this cannot block.
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

    /// Everything the spawn would set, without spawning anything.
    ///
    /// Read back off the very `Command` [`start`] would run, so what is asserted is the
    /// environment a child comes up in and not a second copy of it written out here — which would
    /// go on passing after somebody changed the spawn.
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
        // The trap. balthasar reads the agent out of the connecting peer's initial environment
        // block, so this is the one place it can be set — and set to the parent's, both sessions
        // would open the same `<run>/<agent>/memory.db` and file one another's working notes.
        let minted = minted();
        let environ = spawn_environment(&minted);
        assert_eq!(
            environ.get(crate::balthasar::AGENT).and_then(Clone::clone),
            Some("iota-mu".to_owned()),
            "the child came up as somebody else"
        );
        assert_ne!(
            environ.get(crate::balthasar::AGENT).and_then(Clone::clone),
            Some("alpha-rho".to_owned())
        );
    }

    #[test]
    fn a_child_belongs_to_the_run_that_started_it() {
        // Not to one of its own. A child that minted a run would land in a different `crew`, file
        // its memory where the parent will not look, and read as `Root` to the session that
        // started it — every relation the last six stages built, quietly inert.
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
        // `env_remove` shows up as a name with no value, which is what tells the spawn to unset
        // it rather than pass it on. Inherited, the child would keep its transcript in a process
        // that dies with the coordinator — and a session that cannot record does not start.
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
        // Read whole or not at all: a name from the flag and a sentence from somewhere else
        // describes a role nobody declared. melchior packs both into one string; magi carries it.
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
        // Rather than defaulted to something. A child watching a pid nobody named would be the
        // orphan the whole arrangement exists to prevent: it answers, and nothing can end it.
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
        // The one failure a person will actually meet. It must name the program rather than
        // arriving as a descriptor that would not parse.
        let why = mint("melchior-that-is-not-installed", None, None)
            .expect_err("nothing named the child")
            .to_string();
        assert!(why.contains("melchior-that-is-not-installed"), "{why}");
    }

    #[test]
    fn a_child_that_dies_at_once_is_reported_rather_than_announced() {
        // `magi fork` prints an id, and printing one is a promise that something answers to it.
        // Without the watch the promise gets made for a process that is already gone -- and the
        // person who forked it finds out by addressing a session that never existed.
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
        // The claim the whole file rests on, and the only way to check it is to look at a real
        // child's own initial block. `setenv` in this process would not appear there, which is
        // exactly why balthasar cannot be told an agent any later than this.
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
        // Both on argv rather than in the environment, because both are this spawn's alone: a
        // shell the child starts must not inherit a prompt, and must not inherit a lifetime.
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
            ["--tied", "4242", "read the diff"]
        );
    }

    /// A runnable stand-in for the magi a fork would start, written into `dir`.
    ///
    /// A script rather than a mock, because what is under test is the spawn: the argv, the
    /// environment block the kernel writes, and whether this process notices one that exits.
    /// Nothing in this process could prove any of the three.
    ///
    /// The directory belongs to the caller and takes itself away — a helper of its own that made
    /// one under the temporary directory left a copy per run behind, which is the leak
    /// [`magi_model::scratch::Scratch`] exists to end.
    fn a_harness(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("magi");
        let mut file = std::fs::File::create(&path).expect("write the stand-in");
        // The probe below runs it, so it must have a way out that does nothing: without one, the
        // check that the file is executable would itself be the fixture's whole behaviour.
        write!(file, "#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\n{body}").expect("write");
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        // `ETXTBSY`: between a `fork` on another thread and its `exec` the child holds every
        // descriptor this one had, including the file just written. One successful exec proves
        // nothing is still holding it, and nothing writes it again after this.
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
