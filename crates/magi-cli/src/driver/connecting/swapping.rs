//! Pointing the loop at somebody else, against two real sockets.
//!
//! Two facts are worth a socket rather than an assertion about one. What a session is *asked for*
//! on a swap decides whether the peer's history arrives at all, and whether the UI claims a screen
//! on a session that already has one. Neither is visible from inside the loop; both are the first
//! frame it writes.

use super::*;
use magi_model::scratch::Scratch;
use magi_proto::UiCommand;
use std::path::PathBuf;
use std::sync::Mutex;

/// Everything one side of a socket was sent, in order.
type Heard = Arc<Mutex<Vec<UiCommand>>>;

/// Answer on `path`, recording every frame and holding the connection open.
async fn recorder(path: &std::path::Path) -> Heard {
    let listener = magi_ipc::bind(path).await.expect("bind");
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&heard);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let (read_half, _write) = stream.into_split();
                let mut reader = FrameReader::new(read_half);
                while let Ok(command) = reader.read::<UiCommand>().await {
                    seen.lock().expect("heard").push(command);
                }
            });
        }
    });
    heard
}

/// Wait until `heard` holds at least `count` frames, or give up.
async fn settled(heard: &Heard, count: usize) -> Vec<UiCommand> {
    for _ in 0..200 {
        let seen = heard.lock().expect("heard").clone();
        if seen.len() >= count {
            return seen;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    heard.lock().expect("heard").clone()
}

/// What the loop asked each of the two sessions for.
struct Swapped {
    own: Heard,
    peer: Heard,
    commands: mpsc::Sender<UiCommand>,
    /// Held open, not read: the loop's reader task ends the moment nobody is taking events, and
    /// a connection that ended for that reason would prove nothing about the one under test.
    _events: mpsc::Receiver<HarnessEvent>,
    /// Held for the same reason: a watch with no sender answers `changed` immediately and for
    /// ever, which is the driver having gone rather than the screen having moved.
    _target: watch::Sender<PathBuf>,
    /// The directory both sockets are in, removed when this is dropped.
    ///
    /// It used to unlink the two paths by name here instead, which left them behind whenever
    /// [`swap`] panicked between binding the first and returning this — and a week of that is a
    /// temporary directory full of sockets. The guard also covers the unwind, which the two
    /// `remove_file` calls did and a last line would not.
    _at: Scratch,
}

/// Attach to our own session at `from`, then point the screen at a peer.
///
/// **Short names.** A unix socket path may not exceed `SUN_LEN` — 108 bytes — and under
/// `gate-hermetic` the whole run already sits in a private temporary directory. One directory
/// holding `own` and `peer` is shorter than two paths that each spell out both.
async fn swap(name: &str, from: Cursor) -> Swapped {
    let at = Scratch::new("sw", name);
    let own_at = at.join("own.sock");
    let peer_at = at.join("peer.sock");
    let own = recorder(&own_at).await;
    let peer = recorder(&peer_at).await;

    let (events, events_rx) = mpsc::channel(64);
    let (commands, command_rx) = mpsc::channel(32);
    let (target, watching) = watch::channel(own_at.clone());
    let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    tokio::spawn(connection_loop(
        own_at.clone(),
        watching,
        events,
        command_rx,
        from,
        attached,
    ));

    settled(&own, 1).await;
    target.send(peer_at.clone()).expect("still watching");
    settled(&peer, 1).await;
    Swapped {
        own,
        peer,
        commands,
        _events: events_rx,
        _target: target,
        _at: at,
    }
}

/// The one number that decides whether a peer appears to have said anything.
///
/// A session answers an attach by keeping back as many entries as the cursor claims are already
/// held. Carried across a swap, the count from our own session withholds exactly that much of the
/// peer's history — and on a screen that has been open a while, that is all of it. Nothing errors;
/// the agent simply looks like it has never spoken.
#[tokio::test]
async fn a_peer_is_asked_for_its_history_from_the_beginning() {
    let swapped = swap("cursor", Cursor(400)).await;

    let asked = swapped.own.lock().expect("heard").clone();
    assert!(
        matches!(asked.first(), Some(UiCommand::Attach { from_cursor, .. }) if *from_cursor == Cursor(400)),
        "our own session is rejoined where we left it: {asked:?}"
    );

    let asked = swapped.peer.lock().expect("heard").clone();
    match asked.first() {
        Some(UiCommand::Attach { from_cursor, .. }) => {
            assert_eq!(*from_cursor, Cursor::ZERO, "{asked:?}");
        }
        other => panic!("expected an attach, got {other:?}"),
    }
}

/// Two UIs holding a screen on one session is a tool given rows in a terminal it will never
/// draw in. The peer's own UI is the one that lives there.
#[tokio::test]
async fn only_our_own_session_is_told_there_is_a_screen_here() {
    let swapped = swap("draws", Cursor::ZERO).await;

    let asked = swapped.own.lock().expect("heard").clone();
    assert!(
        matches!(asked.first(), Some(UiCommand::Attach { draws, .. }) if *draws),
        "{asked:?}"
    );
    assert!(
        asked.iter().any(|c| matches!(c, UiCommand::Sized { .. })),
        "our own session is told how wide the window is: {asked:?}"
    );

    let asked = swapped.peer.lock().expect("heard").clone();
    assert!(
        matches!(asked.first(), Some(UiCommand::Attach { draws, .. }) if !*draws),
        "{asked:?}"
    );
    assert!(
        !asked.iter().any(|c| matches!(c, UiCommand::Sized { .. })),
        "and a peer is not told the width of a window it cannot use: {asked:?}"
    );
}

/// Leaving is said, not left to the socket closing: a session counts its screens.
#[tokio::test]
async fn the_session_we_left_is_told_we_have_gone() {
    let swapped = swap("detach", Cursor::ZERO).await;
    let asked = settled(&swapped.own, 2).await;
    assert!(
        asked.iter().any(|c| matches!(c, UiCommand::Detach)),
        "{asked:?}"
    );
}

/// The race the UI's own gate cannot close: a command queued a moment before the screen moved.
#[tokio::test]
async fn a_command_meant_for_our_own_session_never_reaches_a_peer() {
    let swapped = swap("gate", Cursor::ZERO).await;
    swapped
        .commands
        .send(UiCommand::SubmitPrompt {
            text: "this was meant for my own session".into(),
            aside: String::new(),
        })
        .await
        .expect("queued");
    // Long enough for a frame to cross a unix socket in the same process many times over.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let asked = swapped.peer.lock().expect("heard").clone();
    assert!(
        !asked
            .iter()
            .any(|c| matches!(c, UiCommand::SubmitPrompt { .. })),
        "{asked:?}"
    );
}
