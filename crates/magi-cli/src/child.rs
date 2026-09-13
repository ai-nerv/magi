//! A session with no terminal: what `magi --headless` is, and how it ends. It submits whatever it
//! was given and then sits there being reachable; the first screen that points itself here draws.
//! It detaches from its own socket afterwards, because magi asks "is anybody attached" by counting
//! subscribers and one that cannot answer turns every gated tool into a stall — so an unattended
//! child gets what `magi -p` gets. It watches the pid on `--tied` rather than using
//! `PR_SET_PDEATHSIG`, which follows the immediate parent — for a fork the `magi fork` that exits a
//! moment later — and watches its start time too, because pids come round again.

use anyhow::Result;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{AgentStatus, Cursor, HarnessEvent, Phase, UiCommand};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// How often the parent is looked for — a corpse check rather than a heartbeat.
const LOOK: Duration = Duration::from_secs(1);

/// A bound rather than a wait: a stalled session must not keep the child from coming up reachable.
const TAKEN: Duration = std::time::Duration::from_secs(10);

/// An attach position past every entry there could be: this connection is only here to add to it.
const FROM_END: Cursor = Cursor(u64::MAX);

/// Serve this session until somebody stops it or the session that forked it goes. `parent` is the
/// pid on `--tied`, `None` for a root; dropping `started` tells melchior the session is over.
pub async fn run(
    socket: &Path,
    prompt: Option<String>,
    started: Option<(crate::melchior::Melchior, std::path::PathBuf)>,
    parent: Option<u32>,
    phase: watch::Receiver<AgentStatus>,
) -> Result<()> {
    let mut layer = started.map(|(layer, _at)| layer);
    // Said once, because a session with no terminal has no other way to say what it is called.
    if let Some(named) = layer.as_ref().map(|layer| layer.named.as_str())
        && !named.is_empty()
    {
        println!("{named}");
    }
    // Whether it was given work at birth: what tells `idle` (waiting to be told) from `finished`
    // (the work it was given is done) once its turn ends.
    let had_prompt = prompt.is_some();
    // Coming up, before the prompt lands: the one moment `starting` is true.
    if let Some(layer) = layer.as_mut() {
        layer.doing(false, 0, None, Phase::Starting, None);
    }
    if let Some(prompt) = prompt {
        // Not fatal: a child whose prompt did not land is still a session somebody can attach to.
        if let Err(why) = ask(socket, prompt).await {
            eprintln!("magi: this session could not be given its prompt: {why}");
        }
    }
    park(socket, &mut layer, parent, phase, had_prompt).await;
    Ok(())
}

/// Hand the session its prompt, then let go of the socket. It waits to see the prompt land: the
/// session `select!`s between the commands it has read and the task reading them, and a client that
/// wrote and closed in one breath loses that coin toss half the time.
async fn ask(socket: &Path, prompt: String) -> Result<()> {
    let stream = magi_ipc::connect(socket).await?;
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: FROM_END,
            // No terminal here, so no rows are reserved for a surface nothing could draw.
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
    // Whatever the session publishes first, bounded so a stalled session does not block parking.
    let landed = tokio::time::timeout(TAKEN, reader.read::<HarnessEvent>()).await;
    writer.write(&UiCommand::Detach).await?;
    if landed.is_err() {
        anyhow::bail!("the session did not take the prompt within {TAKEN:?}");
    }
    Ok(())
}

/// Stay up until somebody stops this session, the session that forked it goes, or a signal comes —
/// handled rather than defaulted so the transcript reaches balthasar. A root has no parent, so that
/// arm is absent rather than watching nothing: [`still_running`] would end it on the first tick.
async fn park(
    socket: &Path,
    layer: &mut Option<crate::melchior::Melchior>,
    parent: Option<u32>,
    mut phase: watch::Receiver<AgentStatus>,
    had_prompt: bool,
) {
    let mut heard = layer
        .as_mut()
        .and_then(crate::melchior::Melchior::hearing)
        .map(listening);
    let watching = parent.map(|pid| (pid, started_at(pid)));
    let mut looking = watching.as_ref().map(|_| tokio::time::interval(LOOK));
    let mut ended = signal();

    // Its own phase, reported mechanically so a coordinator knows what it is doing without the
    // model choosing to say. `working_since` makes the timer live; `has_worked` is what turns the
    // first idle after a turn into `finished` rather than `idle`.
    let mut has_worked = false;
    let mut working_since: Option<Instant> = None;
    // A slow beat so `working` shows a climbing timer between the rare status changes.
    let mut beat = tokio::time::interval(Duration::from_secs(2));
    // The latest edge worth a turn, held until this session is idle and off the wake cooldown, so
    // a burst of finishes coalesces into one turn and a mutual watch cannot spin. The turn reads
    // the whole crew, so holding only the latest occasion loses nothing.
    let mut pending_wake: Option<String> = None;
    let mut last_wake: Option<Instant> = None;
    // Each watched child's last phase, so a coordinator gone quiet knows whether it still waits.
    let mut children: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let status = phase.borrow().clone();
    let (mut phase_now, working_for) =
        derive(&status, had_prompt, &mut has_worked, &mut working_since);
    announce(layer, phase_now, working_for, &children);

    loop {
        tokio::select! {
            said = next(&mut heard) => match said {
                Some(crate::melchior::Heard::Stopped) => break,
                // The pipe closed: melchior is gone, so nothing can reach this session or end it.
                None => break,
                // A watched agent moved. A child finishing or blocking is this session's to act on;
                // its phase is tracked either way, so this session can say what it still waits on.
                Some(crate::melchior::Heard::Signal { from, kind, kin, cause }) => {
                    if kin == "child" {
                        children.insert(from.clone(), kind.clone());
                        announce(layer, phase_now, working_since.map_or(0, |t| t.elapsed().as_secs()), &children);
                    }
                    if let Some(occasion) = wake_prompt(&kin, &kind, &from, cause.as_deref()) {
                        pending_wake = Some(occasion);
                        flush_wake(socket, phase_now, &mut pending_wake, &mut last_wake).await;
                    }
                }
                // The one message allowed to reach a session mid-turn. Working: stop the turn so it
                // attends sooner; the queued occasion then wakes it when the turn ends.
                Some(crate::melchior::Heard::Message { who, sort, text })
                    if crate::melchior::interrupts(&sort) =>
                {
                    pending_wake = Some(urgent(&who, &sort, &text));
                    if phase_now == Phase::Working {
                        interrupt(socket).await;
                    } else {
                        flush_wake(socket, phase_now, &mut pending_wake, &mut last_wake).await;
                    }
                }
                Some(_) => {}
            },
            // The host's status changed. Report it, and when a turn has just ended take up anything
            // that came in while it ran.
            changed = phase.changed() => {
                if changed.is_err() {
                    break;
                }
                let status = phase.borrow_and_update().clone();
                let (next, working_for) =
                    derive(&status, had_prompt, &mut has_worked, &mut working_since);
                phase_now = next;
                announce(layer, next, working_for, &children);
                flush_wake(socket, phase_now, &mut pending_wake, &mut last_wake).await;
            }
            // Keeps the working timer moving, and catches a wake deferred by the cooldown.
            _ = beat.tick() => {
                if let Some(since) = working_since {
                    announce(layer, Phase::Working, since.elapsed().as_secs(), &children);
                }
                flush_wake(socket, phase_now, &mut pending_wake, &mut last_wake).await;
            }
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
    report(layer, Phase::Gone, None, 0);
}

/// The occasion to wake this session on, or `None` for a signal it should only observe. A child or
/// a watched agent finishing or hitting trouble is what a coordinator resumes for; a parent's edge,
/// and a mere start or working tick, are not.
pub(crate) fn wake_prompt(kin: &str, kind: &str, from: &str, cause: Option<&str>) -> Option<String> {
    if kin != "child" && kin != "watched" {
        return None;
    }
    let whose = if kin == "child" {
        "A subagent you started"
    } else {
        "An agent you are watching"
    };
    match kind {
        "finished" => Some(format!(
            "{whose}, `{from}`, has finished. Read your crew and inbox with the `agent` tool — \
             `crew` for who is still going, `inbox` for what they sent. Your summary must come only \
             from what `inbox` actually holds: if a child sent nothing, say it reported nothing — do \
             not invent, recall, or guess its findings. If everyone you were waiting on is done, \
             write that short summary and stop; otherwise keep waiting. Do not spawn new agents, \
             change roles, or look for files."
        )),
        "blocked" => Some(format!(
            "{whose}, `{from}`, is blocked{}. Say in one line what should happen next. Do not spawn \
             new agents or change roles.",
            cause.map(|why| format!(": {why}")).unwrap_or_default()
        )),
        _ => None,
    }
}

/// The least time between wakes: a burst of finishes coalesces into one turn, and two agents that
/// watch each other cannot spin faster than this.
pub(crate) const WAKE_COOLDOWN: Duration = Duration::from_secs(2);

/// Take up the queued occasion, if there is one and it is time: only while idle (one turn at a
/// time) and not within [`WAKE_COOLDOWN`] of the last wake. Called from every arm that could make
/// a wake due, so a deferred one lands on the next beat rather than being lost.
async fn flush_wake(
    socket: &Path,
    phase_now: Phase,
    pending_wake: &mut Option<String>,
    last_wake: &mut Option<Instant>,
) {
    if phase_now == Phase::Working || pending_wake.is_none() {
        return;
    }
    if last_wake.is_some_and(|at| at.elapsed() < WAKE_COOLDOWN) {
        return;
    }
    if let Some(occasion) = pending_wake.take() {
        *last_wake = Some(Instant::now());
        wake(socket, &occasion).await;
    }
}

/// Give this session a turn, on its own socket, without staying attached — the same brief connection
/// [`ask`] makes. Best effort: a wake that cannot land is a coordinator that stays parked, not a crash.
async fn wake(socket: &Path, occasion: &str) {
    if let Err(why) = ask(socket, occasion.to_owned()).await {
        eprintln!("magi: a signal could not wake this session: {why}");
    }
}

/// The occasion to take up an urgent message on.
fn urgent(who: &str, sort: &str, text: &str) -> String {
    let text: String = text.chars().take(200).collect();
    format!(
        "An urgent `{sort}` from `{who}`: {text}. Read your inbox with the agent tool and attend to it."
    )
}

/// Stop this session's running turn, over its own socket, without staying attached. Best effort: a
/// turn that cannot be reached ends on its own soon enough.
async fn interrupt(socket: &Path) {
    let Ok(stream) = magi_ipc::connect(socket).await else {
        return;
    };
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);
    let attach = UiCommand::Attach {
        session: None,
        from_cursor: FROM_END,
        draws: false,
    };
    if writer.write(&attach).await.is_err() {
        return;
    }
    let _ = reader.read::<HarnessEvent>().await;
    let _ = writer.write(&UiCommand::Interrupt).await;
    let _ = writer.write(&UiCommand::Detach).await;
}

/// The phase a headless session is in, from its host's status and whether it was given work: a turn
/// running is `working`; the first quiet after one is `finished` for a session that had a task,
/// `idle` for one still waiting to be told.
fn derive(
    status: &AgentStatus,
    had_prompt: bool,
    has_worked: &mut bool,
    working_since: &mut Option<Instant>,
) -> (Phase, u64) {
    match status {
        AgentStatus::Working { .. } | AgentStatus::Retrying { .. } => {
            *has_worked = true;
            let since = working_since.get_or_insert_with(Instant::now);
            (Phase::Working, since.elapsed().as_secs())
        }
        AgentStatus::Idle => {
            *working_since = None;
            if had_prompt && *has_worked {
                (Phase::Finished, 0)
            } else {
                (Phase::Idle, 0)
            }
        }
    }
}

/// Report a phase, but a session gone quiet while children are still going reads as `waiting on`
/// them, not `finished` — the state a coordinator sits in between delegating and gathering.
fn announce(
    layer: &mut Option<crate::melchior::Melchior>,
    base: Phase,
    working_for: u64,
    children: &std::collections::BTreeMap<String, String>,
) {
    let (phase, cause) = match base {
        Phase::Idle | Phase::Finished => match waiting_on(children) {
            Some(on) => (Phase::Waiting, Some(on)),
            None => (base, None),
        },
        _ => (base, None),
    };
    report(layer, phase, cause.as_deref(), working_for);
}

/// How many watched children have not reached an end, as a cause line, or `None` when all are done.
fn waiting_on(children: &std::collections::BTreeMap<String, String>) -> Option<String> {
    let live = children
        .values()
        .filter(|phase| !matches!(phase.as_str(), "finished" | "gone" | "blocked"))
        .count();
    (live > 0).then(|| format!("{live} subagent{}", if live == 1 { "" } else { "s" }))
}

/// Tell melchior what this session is doing. `waiting` is `None`: a parked child does not track its
/// own inbox, and must not wipe it to zero.
fn report(
    layer: &mut Option<crate::melchior::Melchior>,
    phase: Phase,
    cause: Option<&str>,
    working_for: u64,
) {
    if let Some(layer) = layer.as_mut() {
        layer.doing(
            matches!(phase, Phase::Working),
            working_for,
            None,
            phase,
            cause,
        );
    }
}

/// The next look for a parent, or `pending` for a session with none — see [`next`].
async fn tick(looking: &mut Option<tokio::time::Interval>) {
    match looking {
        Some(looking) => {
            looking.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// The reader is handed over already buffered; wrapping the raw pipe again drops what is in it.
fn listening(
    reading: std::io::BufReader<std::process::ChildStdout>,
) -> tokio::sync::mpsc::Receiver<crate::melchior::Heard> {
    let (said, heard) = tokio::sync::mpsc::channel(16);
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in reading.lines().map_while(Result::ok) {
            // A line this build cannot read is a newer melchior, not a reason to stop reading.
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

/// The next thing melchior said, or `pending`: a `recv` with no sender resolves at once and spins.
async fn next(
    heard: &mut Option<tokio::sync::mpsc::Receiver<crate::melchior::Heard>>,
) -> Option<crate::melchior::Heard> {
    match heard {
        Some(heard) => heard.recv().await,
        None => std::future::pending().await,
    }
}

/// Both signals, so a transcript is not lost depending on whether it was `kill` or ctrl-c.
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

/// Whether the session that forked this one is still the process it was: a missing directory or a
/// changed start time are both "gone", the second being a pid that came round again.
fn started_at(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat.rsplit_once(')')?.1;
    fields.split_whitespace().nth(19).map(ToOwned::to_owned)
}

/// When the process with this id started: field 22 of `/proc/<pid>/stat`, read from after the last
/// `)`, because the command name before it may contain spaces and brackets.
fn still_running(pid: u32, since: Option<&str>) -> bool {
    match (started_at(pid), since) {
        (Some(now), Some(then)) => now == then,
        // Nothing was readable at the start either, so the directory alone has to answer.
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
        // A child that got this wrong sits in the directory answering to a name nobody can stop.
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
        // Pids recycle, and on a machine with a small `pid_max` they recycle in minutes.
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
        // `/proc/<pid>/stat` puts the command in brackets in field two, unescaped.
        let me = std::process::id();
        let mine = started_at(me).expect("a start time");
        assert!(
            mine.parse::<u64>().is_ok(),
            "field 22 is a number of clock ticks: {mine}"
        );
    }

    /// [`still_running`] says "gone" for a pid it cannot read — fatal for a root, which has none.
    #[tokio::test(start_paused = true)]
    async fn a_root_is_not_ended_by_the_parent_it_does_not_have() {
        let waited = tokio::time::timeout(
            LOOK * 4,
            park(
                Path::new("/nonexistent.sock"),
                &mut None,
                None,
                watch::channel(AgentStatus::Idle).1,
                false,
            ),
        )
        .await;
        assert!(
            waited.is_err(),
            "a headless magi with nothing above it ended itself"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_child_whose_parent_has_gone_stops_parking() {
        // The other half, so the arm above is not simply switched off.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("`true` runs");
        let pid = child.id();
        child.wait().expect("reaped");
        let waited = tokio::time::timeout(
            LOOK * 4,
            park(
                Path::new("/nonexistent.sock"),
                &mut None,
                Some(pid),
                watch::channel(AgentStatus::Idle).1,
                false,
            ),
        )
        .await;
        assert!(
            waited.is_ok(),
            "the child outlived the session that forked it"
        );
    }

    #[test]
    fn a_parent_nothing_could_be_read_about_is_not_running() {
        // Pid 0 cannot be stat'd, and "cannot tell" treated as "carry on" never ends.
        assert!(started_at(0).is_none());
        assert!(!still_running(0, None));
    }

    #[test]
    fn a_coordinator_waits_while_children_are_still_going() {
        let mut kids = std::collections::BTreeMap::new();
        assert_eq!(waiting_on(&kids), None, "no children, nothing to wait on");
        kids.insert("a".to_owned(), "working".to_owned());
        kids.insert("b".to_owned(), "finished".to_owned());
        assert_eq!(
            waiting_on(&kids).as_deref(),
            Some("1 subagent"),
            "one still going"
        );
        kids.insert("a".to_owned(), "finished".to_owned());
        assert_eq!(waiting_on(&kids), None, "all done, no longer waiting");
    }

    #[test]
    fn only_a_child_finishing_or_blocking_is_a_reason_to_wake() {
        // A coordinator resumes for its own children reaching an end — not for a parent's edge,
        // and not for a child merely starting or working.
        assert!(wake_prompt("child", "finished", "psi", None).is_some());
        assert!(wake_prompt("child", "blocked", "psi", Some("declined")).is_some());
        assert!(wake_prompt("watched", "finished", "far", None).is_some());
        assert!(wake_prompt("child", "working", "psi", None).is_none());
        assert!(wake_prompt("child", "idle", "psi", None).is_none());
        assert!(wake_prompt("parent", "finished", "lead", None).is_none());
        assert!(
            wake_prompt("child", "blocked", "psi", Some("run declined"))
                .unwrap()
                .contains("run declined")
        );
    }
}
