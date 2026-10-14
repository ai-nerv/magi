//! `magi fork` against the real melchior and the real balthasar, started the way a session starts
//! it: the parent's own `spawn` tool runs it, so melchior sees a fork its session made. The model
//! is a script in front of the real melchior on the sessions' `PATH`: it answers `models` and
//! `ask` and hands every other verb to melchior. The claim is that a separate process, named by a
//! separate program, comes up in the same run as its parent and files its memory in a directory of
//! its own. Skipped when either sibling is missing, and when what is installed is too old to be
//! asked. melchior's notes beside a socket are its own file format and are not read here; `crew`
//! and `list` are asked instead, and a stop goes through a session's own `agent` tool. The store
//! is magi's own.

use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, Entry, HarnessEvent, ToolResult, UiCommand};
use magi_testkit::mind::{MODEL, call_lines, stop_line, stopped_line, text_line};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for something a spawned session does on its own clock.
const PATIENCE: Duration = Duration::from_secs(40);

/// How long to wait for a session's first line before calling it stuck: far above [`PATIENCE`], and
/// the difference between a suite that fails and a suite that never returns.
const HANGING: Duration = Duration::from_secs(180);

/// What a parent is first asked, and the words the script answers with a `spawn`.
const FORK: &str = "fork a reviewer";

/// What a session is asked when it should stop the child, and the words the script answers with an
/// `agent` call naming it.
const STOP: &str = "stop the reviewer";

/// The script's tool call ids: a later ask carrying one is past that call.
const FORK_CALL: &str = "call-fork";
const STOP_CALL: &str = "call-stop";

/// Held for the whole of each test here, so only one is running sessions at a time. Each stands up
/// two or three, and a session is a magi, a melchior and a balthasar with a store apiece.
static ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`ALONE`], ignoring a poisoning left by some other test's failure.
fn alone() -> std::sync::MutexGuard<'static, ()> {
    ALONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Where a sibling is on `PATH`, the way magi looks for one.
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .map(|dir| Path::new(dir).join(program))
        .find(|it| it.exists())
}

/// A workspace to run in, or nothing when this machine cannot answer the question. Installed is not
/// new enough: probed rather than read off a version, because none of them carries one.
fn ready(name: &str) -> Option<Scratch> {
    let (Some(melchior), Some(_)) = (on_path("melchior"), on_path("balthasar")) else {
        magi_testkit::live::unavailable("melchior and balthasar are not both on PATH");
        return None;
    };
    let dir = workspace(name, &melchior);
    // The refusal, not the answer: `crew` with nothing running names nobody and would skip everywhere.
    if asked(&dir, "probe", "crew", None)
        .1
        .contains("not one of agent's verbs")
    {
        magi_testkit::live::unavailable("melchior predates the run-scoped crew");
        return None;
    }
    Some(dir)
}

/// A project to run sessions in. A checkout, because balthasar scopes a directory that is not one by
/// walking up for a `.git`; short names, because a socket path may not exceed `SUN_LEN`; settling,
/// because a balthasar tied to a magi notices it die by looking, with its sqlite still open. The
/// shipped configuration, choosing the script's model, and the script in `b/`.
fn workspace(name: &str, melchior: &Path) -> Scratch {
    let dir = Scratch::new("mf", name).settling();
    for under in ["p", "r", "c/magi", "d", "b"] {
        std::fs::create_dir_all(dir.join(under)).expect("mkdir");
    }
    std::fs::create_dir_all(dir.join("p/.git")).expect("mkdir");
    let _ = dir.private("r");
    install_config(&dir.join("c/magi"));
    let script = dir.join("b/melchior");
    std::fs::write(&script, model(melchior)).expect("write the script");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    while let Err(why) = Command::new(&script).arg("verbs").output() {
        assert_eq!(why.raw_os_error(), Some(26), "{why}");
        std::thread::sleep(Duration::from_millis(5));
    }
    dir
}

/// The shipped configuration, with the script's model chosen and no memory helper asking it.
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
    let init = into.join("init.lua");
    let mut source = std::fs::read_to_string(&init).expect("the installed entry point");
    source.push_str(&format!(
        "\nmagi.model = \"{MODEL}\"\nmagi.helpers = {{ memory = false }}\n"
    ));
    std::fs::write(&init, source).expect("write init");
}

/// The script standing in for the model, in front of `melchior`. It forks when asked to [`FORK`]
/// by a session that has `spawn`, stops the id in `b/who` when asked to [`STOP`], and otherwise
/// says it is done.
fn model(melchior: &Path) -> String {
    let card = serde_json::json!({
        "id": MODEL, "provider": "fake", "name": "one",
        "api": "anthropic-messages", "context_window": 200_000,
        "max_output": 4096, "reasons": true, "ready": true,
    });
    let models = serde_json::json!({ "ok": true, "family": 1, "n": 1, "result": [card] });
    let fork = call_lines(
        FORK_CALL,
        "spawn",
        &serde_json::json!({ "role": "reviewer", "prompt": "look at the diff" }).to_string(),
    );
    let started = serde_json::json!({
        "event": "tool_call_start", "id": STOP_CALL, "name": "agent",
    });
    let args = serde_json::json!({ "event": "tool_call_args", "args": "@" }).to_string();
    let (before, after) = args.split_once('@').expect("a placeholder");
    let (before, after) = (
        format!("{before}{{\\\"verb\\\":\\\"stop\\\",\\\"who\\\":\\\""),
        format!("\\\"}}{after}"),
    );
    format!(
        "#!/bin/sh\n\
         here=$(dirname \"$0\")\n\
         case \"$1\" in\n\
         models) printf '%s\\n' '{models}'; exit 0 ;;\n\
         ask) ;;\n\
         *) exec '{real}' \"$@\" ;;\n\
         esac\n\
         asked=$(cat)\n\
         has() {{ printf '%s' \"$asked\" | grep -qF \"$1\"; }}\n\
         if has '{FORK}' && has '\"name\":\"spawn\"' && ! has '{FORK_CALL}'; then\n\
         printf '%s\\n' '{f0}' '{f1}' '{f2}'\n\
         elif has '{STOP}' && ! has '{STOP_CALL}'; then\n\
         printf '%s\\n' '{started}'\n\
         printf '%s%s%s\\n' '{before}' \"$(cat \"$here/who\")\" '{after}'\n\
         printf '%s\\n' '{tool_use}'\n\
         else\n\
         printf '%s\\n' '{text}' '{stop}'\n\
         fi\n",
        real = melchior.display(),
        f0 = fork[0],
        f1 = fork[1],
        f2 = fork[2],
        tool_use = stopped_line("tool_use"),
        text = text_line("done"),
        stop = stop_line(),
    )
}

/// The binary under test, in this workspace, with nothing of the developer's machine in it and the
/// script ahead of melchior on its `PATH`. `MAGI_API_SOCKET` outranks the three `XDG_` variables —
/// see [`magi_testkit::only_its_own_store`].
fn magi(dir: &Path) -> Command {
    let mut command = Command::new(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")));
    magi_testkit::only_its_own_store(&mut command);
    let path = std::env::var("PATH").unwrap_or_default();
    command
        .current_dir(dir.join("p"))
        .env("PATH", format!("{}:{path}", dir.join("b").display()))
        .env("XDG_RUNTIME_DIR", dir.join("r"))
        .env("XDG_CONFIG_HOME", dir.join("c"))
        .env("XDG_DATA_HOME", dir.join("d"));
    command
}

/// A session with no terminal, the socket it serves its screens on, and the name melchior gave it.
/// Killed on drop, including the drop an `assert!` unwinds through.
struct Session {
    process: Child,
    socket: PathBuf,
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
    fn start(dir: &Path, tag: &str, prompt: &str) -> Self {
        let socket = dir.join("r").join(format!("{tag}.sock"));
        let mut process = magi(dir)
            .arg("--socket")
            .arg(&socket)
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
            socket,
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

    /// A screen on it that may drive it.
    fn screen(&self) -> Screen {
        Screen::open(&self.socket)
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

/// A screen attached to a session with its controller key, the way a person's is.
struct Screen {
    runtime: tokio::runtime::Runtime,
    reader: FrameReader<tokio::net::unix::OwnedReadHalf>,
    writer: FrameWriter<tokio::net::unix::OwnedWriteHalf>,
    /// The transcript as the session handed it over on attach.
    seen: Vec<Entry>,
}

impl Screen {
    fn open(socket: &Path) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let (reader, writer, seen) = runtime.block_on(async {
            let stream = magi_ipc::connect(socket)
                .await
                .expect("the session answers a screen");
            let (read_half, write_half) = stream.into_split();
            let mut reader = FrameReader::new(read_half);
            let mut writer = FrameWriter::new(write_half);
            writer
                .write(&UiCommand::Attach {
                    session: None,
                    from_cursor: Cursor(0),
                    draws: false,
                    key: magi_ipc::read_key(socket),
                })
                .await
                .expect("attach");
            let opened = tokio::time::timeout(PATIENCE, reader.read::<HarnessEvent>())
                .await
                .expect("the session answered a screen")
                .expect("a frame");
            let HarnessEvent::SessionSnapshot { entries, .. } = opened else {
                panic!("a screen got {opened:?} instead of a transcript");
            };
            (reader, writer, entries)
        });
        Self {
            runtime,
            reader,
            writer,
            seen,
        }
    }

    /// Ask the session something, as a person typing at it.
    fn submit(&mut self, text: &str) {
        let Self {
            runtime, writer, ..
        } = self;
        runtime
            .block_on(writer.write(&UiCommand::SubmitPrompt {
                text: text.to_owned(),
                aside: String::new(),
            }))
            .expect("submitted");
    }

    /// What the tool call `call` came back with, once it has.
    fn result_of(&mut self, call: &str) -> ToolResult {
        let held = self.seen.iter().find_map(|entry| match entry {
            Entry::Tool {
                id,
                result: Some(result),
                ..
            } if id.as_str() == call => Some(result.clone()),
            _ => None,
        });
        if let Some(result) = held {
            return result;
        }
        let Self {
            runtime, reader, ..
        } = self;
        runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + PATIENCE;
            loop {
                let event = tokio::time::timeout_at(deadline, reader.read::<HarnessEvent>())
                    .await
                    .unwrap_or_else(|_| panic!("gave up waiting for {call} to come back"))
                    .expect("a frame");
                if let HarnessEvent::ToolCallEnded { id, result, .. } = event
                    && id.as_str() == call
                {
                    return result;
                }
            }
        })
    }

    /// The child this session's `spawn` started: the id the tool handed back.
    fn forked(&mut self) -> String {
        let said = self.result_of(FORK_CALL);
        assert!(!said.is_error, "the spawn failed: {}", said.output);
        let id = said.output.trim().to_owned();
        assert!(!id.is_empty(), "a fork that started something must name it");
        id
    }

    /// Have the session stop `who` with its own `agent` tool, and say what the tool said.
    fn stop(&mut self, dir: &Path, who: &str) -> ToolResult {
        std::fs::write(dir.join("b/who"), who).expect("name who to stop");
        self.submit(STOP);
        self.result_of(STOP_CALL)
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

/// Whether this melchior still mints a child's stop secret into the child's environment: one from
/// before it took the secret on `serve`'s stdin cannot be told it by a magi that hands it over in a
/// file, and refuses the parent's `stop`. Such a melchior also mints for any caller, so asking from
/// here answers; one that checks who is asking refuses, and is not that melchior.
fn mints_into_environment(dir: &Path, parent: &Session) -> bool {
    let mut command = Command::new("melchior");
    command
        .current_dir(dir.join("p"))
        .env("XDG_RUNTIME_DIR", dir.join("r"))
        .args(["fork", "--role", "probe"]);
    as_session(&mut command, parent.id(), "main", parent.process.id());
    let done = command.output().expect("melchior runs");
    serde_json::from_slice::<serde_json::Value>(&done.stdout)
        .is_ok_and(|minted| minted["environment"].get("MAGI_MELCHIOR_TOKEN").is_some())
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
    let runs = std::fs::read_dir(dir.join("p/.balthasar/magi"));
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
    let parent = Session::start(&dir, "a", FORK);
    let child = parent.screen().forked();
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
    let mut parent = Session::start(&dir, "a", FORK);
    let mut screen = parent.screen();
    let child = screen.forked();
    until("the child to be listening", || {
        listening(&dir, parent.id(), &child)
    });
    let theirs = forked_by(parent.process.id()).expect("the child is a running process");

    // The transcript reaches balthasar when a session lets go of it, so both are ended and waited
    // for. The child is stopped; the parent is signalled, the only thing that reaches a headless one.
    let stopped = screen.stop(&dir, &child);
    if stopped.is_error && mints_into_environment(&dir, &parent) {
        magi_testkit::live::unavailable("melchior predates the stop secret on serve's stdin");
        return;
    }
    assert!(
        !stopped.is_error,
        "the parent could not stop its own child: {}",
        stopped.output
    );
    until("the child to finish", || !running(theirs));
    drop(screen);
    parent.end();

    let found = scratches(&dir);
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
    // makes a `stop` refusable. Each side asks through its own `agent` tool, as its model would.
    let parent = Session::start(&dir, "a", FORK);
    let stranger = Session::start(&dir, "b", "a session of my own");
    let mut screen = parent.screen();
    let child = screen.forked();
    until("the child to be listening", || {
        listening(&dir, parent.id(), &child)
    });

    let refused = stranger.screen().stop(&dir, &child);
    assert!(
        refused.is_error && listening(&dir, parent.id(), &child),
        "a session that started nothing stopped somebody: {}",
        refused.output
    );

    let stopped = screen.stop(&dir, &child);
    if stopped.is_error && mints_into_environment(&dir, &parent) {
        magi_testkit::live::unavailable("melchior predates the stop secret on serve's stdin");
        return;
    }
    assert!(
        !stopped.is_error,
        "the session that started it could not end it: {}",
        stopped.output
    );
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
    let parent = Session::start(&dir, "a", FORK);
    let child = parent.screen().forked();
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
