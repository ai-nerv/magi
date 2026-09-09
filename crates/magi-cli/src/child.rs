//! A session with no terminal: what `magi --headless` is, and how it ends.
//!
//! magi's host and its screen were always separable — [`crate::host::start`] binds the socket and
//! serves it as a task, and the process then either draws ([`crate::driver`]) or prints one answer
//! and exits ([`crate::print`]). This is the third front end, and it is the smallest: it does not
//! draw and it does not exit. It submits whatever it was given to get on with, and then sits there
//! being reachable — which is how an agent is started, whether a person typed `--headless` or a
//! coordinator forked it.
//!
//! **Not having a screen is not the same as not being watched.** A UI attaches with `draws: false`
//! when it is looking at somebody else's session, so a session that has no UI of its own is not a
//! special case anywhere — the first screen that points itself here draws, and that is what
//! `alt+,` and `alt+.` are for. Spawning a terminal per child was the alternative, and it would
//! have made a coordinator's fan-out a screenful of windows.
//!
//! # Why it lets go of its own socket
//!
//! The prompt is submitted over the socket like any other, and then this process **detaches** and
//! never dials again. That is deliberate and it is the whole permission story.
//!
//! magi asks "is anybody attached" by counting subscribers on the session's event channel, and a
//! subscriber that cannot answer is worse than none: [`magi_host`] refuses a gated tool outright
//! when nothing is attached, and puts the question up and waits five minutes when something is.
//! A client sitting here for the life of the child would turn every refusal into a stall, and one
//! that answered on its own would be deciding on behalf of a person who has not looked yet.
//!
//! So an unattended child gets exactly what `magi -p` gets: what `magi.allow` covers, and a
//! refusal for everything else. The good half of that arrangement is the one worth saying out
//! loud — **a child somebody is watching can ask them things**, because pressing `>` is what makes
//! the session attached, and the question then goes to the screen that has arrived.
//!
//! # Why a parent is optional and a screen is not
//!
//! There is one mode here and it is "no terminal". What varies is whether anything above this
//! session owns it — a fork has a parent whose pid arrives on `--tied`, and a `magi --headless`
//! somebody started by hand is a root, the same root a `magi` in a terminal is. Making those two
//! flags would have put the same park loop behind two names; making the parent a `None` puts the
//! difference where it actually is, which is one `select!` arm.
//!
//! **What does not vary is the socket.** Both come up through the same prologue and both hand
//! melchior a `--ui`, so both are on every peer's roster with a screen to attach to. That is the
//! whole of what makes a headless agent worth starting: `alt+,` and `alt+.` reach one exactly as
//! they reach a session somebody is sitting in front of.
//!
//! # Why it watches a pid
//!
//! A child that outlives its parent answers to a name nobody can stop: the token a `stop` is
//! checked against was minted by the parent and lives on the parent's socket. balthasar solves the
//! same problem with `PR_SET_PDEATHSIG` and this cannot, because that signal follows the immediate
//! parent — which for a forked child is the `magi fork` that exits a moment later. So the session's
//! own pid comes across on `--tied` and is watched here.
//!
//! Watched with its start time, not by name. Pids come round again, and a directory entry that
//! exists is not proof it is the same process: on a machine with a small `pid_max` a child could
//! outlive its parent by minutes and never notice, because something unrelated had taken the
//! number. The start time is read once at the beginning and compared every tick, so a recycled pid
//! reads as the parent having gone — which it has.

use anyhow::Result;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use std::path::Path;
use std::time::Duration;

/// How often the parent is looked for.
///
/// A second, because this is a corpse check rather than a heartbeat: nothing goes wrong while a
/// child is a second late noticing, and a tighter loop would be a wake-up per second per child on
/// a machine running a fan-out.
const LOOK: Duration = Duration::from_secs(1);

/// How long to wait for the session to show that it has taken the prompt.
///
/// Generous, because what is on the other end is this process's own session and the only thing
/// between them is a commit to a journal. It is a bound rather than a wait: a session that has
/// somehow stopped answering must not keep the child from coming up reachable, since a person who
/// presses `>` can still drive one that never got its opening prompt.
const TAKEN: Duration = std::time::Duration::from_secs(10);

/// An attach position past every entry there could be.
///
/// The same one [`crate::print`] uses, and for the same reason: whatever is already in the
/// transcript is history, and this connection is only here to add to it.
const FROM_END: Cursor = Cursor(u64::MAX);

/// Serve this session until somebody stops it or the session that forked it goes.
///
/// `parent` is the pid on `--tied`, and `None` for a headless magi somebody started by hand —
/// which has no parent to outlive because it is a root. `started` is the layer, taken whole
/// because the pipe it carries is how a `stop` arrives — and dropping it is what tells melchior
/// the session is over.
pub async fn run(
    socket: &Path,
    prompt: Option<String>,
    started: Option<(crate::melchior::Melchior, std::path::PathBuf)>,
    parent: Option<u32>,
) -> Result<()> {
    let mut layer = started.map(|(layer, _at)| layer);
    // Said once, because a session with no terminal has no other way to say what it is called.
    // `magi fork` throws it away — melchior already told it the name when it minted one — but a
    // person who started one of these by hand has nothing else to address it by.
    if let Some(named) = layer.as_ref().map(|layer| layer.named.as_str())
        && !named.is_empty()
    {
        println!("{named}");
    }
    if let Some(prompt) = prompt {
        // Not fatal. A child whose prompt did not land is still a session somebody can attach to
        // and drive, and taking it down would leave a name in the directory for the moment
        // between melchior announcing it and this process exiting.
        if let Err(why) = ask(socket, prompt).await {
            eprintln!("magi: this session could not be given its prompt: {why}");
        }
    }
    park(layer.as_mut(), parent).await;
    Ok(())
}

/// Hand the session its prompt, and let go of the socket again.
///
/// **It waits to see the prompt land, and that is not politeness.** The session serves a
/// connection with a `select!` between the commands it has read and the task doing the reading,
/// and `select!` picks at random among the arms that are ready — so a client that wrote its
/// prompt and closed in the same breath leaves both ready at once, and loses the coin toss about
/// half the time. Measured, not reasoned about: forked children recorded their prompt on some runs
/// and came up with an empty transcript on others, with nothing anywhere saying which had
/// happened. Holding the connection open until an event comes back keeps the reader from finishing
/// at all, so there is nothing for the toss to be between.
async fn ask(socket: &Path, prompt: String) -> Result<()> {
    let stream = magi_ipc::connect(socket).await?;
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: FROM_END,
            // No terminal here. A session told otherwise would reserve rows for a surface
            // nothing in this process could draw or answer.
            draws: false,
        })
        .await?;
    if !matches!(
        reader.read::<HarnessEvent>().await?,
        HarnessEvent::SessionSnapshot { .. }
    ) {
        anyhow::bail!("the session did not open with a snapshot");
    }
    writer
        .write(&UiCommand::SubmitPrompt {
            text: prompt,
            aside: String::new(),
        })
        .await?;
    // Whatever the session publishes first, which is the entry the prompt became. Bounded,
    // because a session that has stopped answering must not keep this one from parking — and
    // reaching the deadline is worth saying out loud rather than treating as arrival.
    let landed = tokio::time::timeout(TAKEN, reader.read::<HarnessEvent>()).await;
    writer.write(&UiCommand::Detach).await?;
    if landed.is_err() {
        anyhow::bail!("the session did not take the prompt within {TAKEN:?}");
    }
    Ok(())
}

/// Stay up and reachable until there is a reason not to be.
///
/// Three reasons, and they are the whole list. Somebody with the right to stop this session did.
/// The session that forked it has gone. Or a signal arrived, which is the only thing that can
/// reach a process with no terminal — and it is handled rather than left to the default so that
/// the transcript reaches balthasar on the way out instead of dying with the process.
///
/// A root headless magi has no second reason, and the arm is *absent* rather than watching
/// nothing: [`still_running`] answers "gone" for a pid it cannot read, which is the right answer
/// for a parent and would end a root the moment the first tick came round.
async fn park(layer: Option<&mut crate::melchior::Melchior>, parent: Option<u32>) {
    let mut heard = layer
        .and_then(crate::melchior::Melchior::hearing)
        .map(listening);
    let watching = parent.map(|pid| (pid, started_at(pid)));
    let mut looking = watching.as_ref().map(|_| tokio::time::interval(LOOK));
    let mut ended = signal();

    loop {
        tokio::select! {
            said = next(&mut heard) => match said {
                Some(crate::melchior::Heard::Stopped) => break,
                // The pipe closed: melchior is gone, so this session is out of the directory and
                // nothing can reach it or end it. That is the orphan, arriving by another door.
                None => break,
                Some(_) => {}
            },
            () = tick(&mut looking) => {
                if let Some((pid, since)) = &watching
                    && !still_running(*pid, since.as_deref())
                {
                    break;
                }
            }
            () = &mut ended => break,
        }
    }
}

/// The next look for a parent, or nothing ever for a session that has none.
///
/// `pending` rather than an interval nobody reads, for the reason [`next`] gives: an arm that is
/// always ready turns the wait into a spin, and a session with no terminal spinning is a core
/// nobody is watching.
async fn tick(looking: &mut Option<tokio::time::Interval>) {
    match looking {
        Some(looking) => {
            looking.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// melchior's pipe, on a thread, as a channel this loop can select on.
///
/// A thread because reading it blocks, which is the same arrangement [`crate::driver`] makes for
/// the same pipe. The reader is handed over already buffered: whatever followed the line that
/// named this session is sitting in it, and wrapping the raw pipe again here would drop it.
fn listening(
    reading: std::io::BufReader<std::process::ChildStdout>,
) -> tokio::sync::mpsc::Receiver<crate::melchior::Heard> {
    let (said, heard) = tokio::sync::mpsc::channel(16);
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in reading.lines().map_while(Result::ok) {
            // A line this build cannot read is a newer melchior, not a reason to stop reading:
            // the next one may well be the stop.
            let Ok(one) = serde_json::from_str::<crate::melchior::Heard>(&line) else {
                continue;
            };
            if said.blocking_send(one).is_err() {
                return;
            }
        }
    });
    heard
}

/// The next thing melchior said, or nothing ever for a session that has no melchior.
///
/// `pending` rather than an empty channel, because a `recv` with no sender resolves at once —
/// which would make the arm above fire every time round and turn the wait into a spin.
async fn next(
    heard: &mut Option<tokio::sync::mpsc::Receiver<crate::melchior::Heard>>,
) -> Option<crate::melchior::Heard> {
    match heard {
        Some(heard) => heard.recv().await,
        None => std::future::pending().await,
    }
}

/// A future that resolves when this process is asked to stop.
///
/// Both signals, because the two ways a person ends something they started by hand are `kill` and
/// ctrl-c, and a child that drained on one and not the other would lose a transcript depending on
/// how it was ended. A handler that cannot be installed is not worth failing over: the default
/// disposition is still to die, which is the outcome this is arranging anyway.
fn signal() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        return Box::pin(std::future::pending());
    };
    Box::pin(async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    })
}

/// When the process with this id started, as the kernel counts it.
///
/// Field 22 of `/proc/<pid>/stat`, taken from after the last `)` because the field before it is
/// the command name and a command name may contain spaces and brackets — which is the classic way
/// to read that file wrongly.
///
/// `None` when there is nothing to read, which is a parent that has already gone. The caller
/// treats that as "no longer running" rather than as "cannot tell".
fn started_at(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat.rsplit_once(')')?.1;
    fields.split_whitespace().nth(19).map(ToOwned::to_owned)
}

/// Whether the session that forked this one is still the process it was.
///
/// Both halves matter. A missing directory is a parent that has exited. A directory whose start
/// time has changed is a pid that came round again and now belongs to somebody else, which for
/// this purpose is the same thing: the session is gone and its socket, its token and its right to
/// stop this child went with it.
fn still_running(pid: u32, since: Option<&str>) -> bool {
    match (started_at(pid), since) {
        (Some(now), Some(then)) => now == then,
        // Nothing was readable at the start either, so there is no start time to disagree with
        // and the directory alone has to answer.
        (found, None) => found.is_some(),
        (None, Some(_)) => false,
    }
}

/// What ends a child, and what does not.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_process_is_read_as_running() {
        let me = std::process::id();
        let since = started_at(me);
        assert!(since.is_some(), "this process has a start time");
        assert!(still_running(me, since.as_deref()));
    }

    #[test]
    fn a_process_that_has_gone_is_read_as_gone() {
        // The whole point of the watch. A child that got this wrong would sit in the directory
        // answering to a name nobody holds the token for.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("`true` runs");
        let pid = child.id();
        let since = started_at(pid);
        child.wait().expect("reaped");
        assert!(
            !still_running(pid, since.as_deref()),
            "the parent was reaped and the child would have stayed up"
        );
    }

    #[test]
    fn a_pid_that_came_round_again_is_not_the_parent() {
        // Pids recycle, and on a machine with a small `pid_max` they recycle in minutes. The
        // directory existing is not the question; whether it is the same process is.
        let me = std::process::id();
        let since = started_at(me).expect("a start time");
        let earlier = format!("{}", since.parse::<u64>().unwrap_or(1).saturating_sub(1));
        assert_ne!(earlier, since, "the fixture must differ from the real one");
        assert!(
            !still_running(me, Some(&earlier)),
            "somebody else's process passed as the parent"
        );
    }

    #[test]
    fn a_start_time_is_read_past_a_command_name_that_contains_brackets() {
        // `/proc/<pid>/stat` puts the command in brackets in field two, unescaped, and a reader
        // that split on whitespace from the left would take a word out of the middle of a name
        // like `(my )( prog)` and compare start times against it forever.
        let me = std::process::id();
        let mine = started_at(me).expect("a start time");
        assert!(
            mine.parse::<u64>().is_ok(),
            "field 22 is a number of clock ticks: {mine}"
        );
    }

    /// **The one that would take a headless magi down a second after it came up.**
    ///
    /// [`still_running`] answers "gone" for a pid it cannot read — right for a parent, and fatal
    /// for a root, which has none. Given the interval unconditionally the arm fires on the first
    /// tick and the process exits cleanly having done nothing, which is a failure with no
    /// evidence anywhere: no error, no log, and a name that was in the directory for a moment.
    ///
    /// Paused time rather than a real second. The bug fires at tick zero, so the clock only has
    /// to move at all — and the honest version of this test parks until the runtime says nothing
    /// else could ever happen.
    #[tokio::test(start_paused = true)]
    async fn a_root_is_not_ended_by_the_parent_it_does_not_have() {
        let waited = tokio::time::timeout(LOOK * 4, park(None, None)).await;
        assert!(
            waited.is_err(),
            "a headless magi with nothing above it ended itself"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_child_whose_parent_has_gone_stops_parking() {
        // The other half, so the arm above is not simply switched off. A child that went on
        // parking is the orphan: a name in the directory answering, and the token to stop it
        // gone with the session that minted it.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("`true` runs");
        let pid = child.id();
        child.wait().expect("reaped");
        let waited = tokio::time::timeout(LOOK * 4, park(None, Some(pid))).await;
        assert!(
            waited.is_ok(),
            "the child outlived the session that forked it"
        );
    }

    #[test]
    fn a_parent_nothing_could_be_read_about_is_not_running() {
        // Pid 0 is not a process anybody can stat. The honest answer is "gone", because a child
        // that treated "cannot tell" as "carry on" would never end.
        assert!(started_at(0).is_none());
        assert!(!still_running(0, None));
    }
}
