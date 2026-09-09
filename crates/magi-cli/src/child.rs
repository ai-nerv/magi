//! A session with no terminal: what `magi --headless` is, and how it ends. It submits whatever it
//! was given and then sits there being reachable; the first screen that points itself here draws.
//! It detaches from its own socket afterwards, because magi asks "is anybody attached" by counting
//! subscribers and one that cannot answer turns every gated tool into a stall — so an unattended
//! child gets what `magi -p` gets. It watches the pid on `--tied` rather than using
//! `PR_SET_PDEATHSIG`, which follows the immediate parent — for a fork the `magi fork` that exits a
//! moment later — and watches its start time too, because pids come round again.

use anyhow::Result;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use std::path::Path;
use std::time::Duration;

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
) -> Result<()> {
    let mut layer = started.map(|(layer, _at)| layer);
    // Said once, because a session with no terminal has no other way to say what it is called.
    if let Some(named) = layer.as_ref().map(|layer| layer.named.as_str())
        && !named.is_empty()
    {
        println!("{named}");
    }
    if let Some(prompt) = prompt {
        // Not fatal: a child whose prompt did not land is still a session somebody can attach to.
        if let Err(why) = ask(socket, prompt).await {
            eprintln!("magi: this session could not be given its prompt: {why}");
        }
    }
    park(layer.as_mut(), parent).await;
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
                // The pipe closed: melchior is gone, so nothing can reach this session or end it.
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
        let waited = tokio::time::timeout(LOOK * 4, park(None, None)).await;
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
        let waited = tokio::time::timeout(LOOK * 4, park(None, Some(pid))).await;
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
}
