//! What happens when another session says something, against a real session on a real socket.
//!
//! The rule under test is invisible when it is wrong. An arrival is committed either way, so a
//! session that should have answered and did not looks exactly like one with nothing to say:
//! the message is in the transcript, the screen is idle, and nothing anywhere reports a problem.
//!
//! That is how `ask` came to be a one-way trip. It sends a `question`, which woke the receiver;
//! `reply` sends an `answer`, which woke nobody — so two agents got one exchange and stopped.

use magi_host::session::Session;
use magi_host::turn::Backend;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, SessionId, UiCommand};
use magi_testkit::Mind;

fn backend(mind: &Mind) -> Backend {
    Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        cwd: std::env::temp_dir(),
        model: "fake/one".to_owned(),
        // A real path to a real program, because the worker spawns it: a backend that cannot be
        // asked cannot answer at all, and the failure would read as "the arrival did not wake
        // it" rather than "there was nothing to wake".
        mind: mind.program().display().to_string(),
        wants: magi_proto::ask::Wants::default(),
        context_window: Some(200_000),
        system: None,
        confine: false,
        grants: Vec::new(),
        environ: std::collections::BTreeMap::new(),
    }
}

/// A session serving on its own socket, and the path to reach it at.
async fn serving(name: &str, mind: &Mind) -> std::path::PathBuf {
    // Short: a scratch directory path does not fit in `SUN_LEN`.
    let path = std::env::temp_dir().join(format!("magi-arr-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let dir = std::env::temp_dir().join(format!("magi-arr-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let session = Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("s");
    let listener = magi_ipc::bind(&path).await.expect("bind");
    let backend = backend(mind);
    tokio::spawn(async move {
        let _ = magi_host::serve_catalog(
            listener,
            session,
            Some(backend),
            magi_host::catalog::Catalog::empty(),
        )
        .await;
    });
    path
}

/// Attach, hand over one arrival, and report every event that follows within `patience`.
///
/// The second half of the answer is whether the mind was asked at all. "No turn ran" has to
/// mean nothing was spawned, not merely that no event happened to arrive before a timeout.
async fn arrival_of(sort: &str, name: &str) -> (Vec<HarnessEvent>, bool) {
    let mind = Mind::answering(&format!("arr-{name}"), "thanks");
    let path = serving(name, &mind).await;
    let stream = magi_ipc::connect(&path).await.expect("connect");
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor(0),
            draws: false,
        })
        .await
        .expect("attach");
    let _snapshot: HarnessEvent = reader.read().await.expect("a snapshot");

    writer
        .write(&UiCommand::Arrived {
            who: "magi/main/beta-nu".to_owned(),
            kin: "main".to_owned(),
            sort: sort.to_owned(),
            text: "the parser is done".to_owned(),
        })
        .await
        .expect("the arrival goes");

    // Long enough for a turn that is going to happen, and spent in full when one is not.
    let patience = std::time::Duration::from_secs(3);
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + patience;
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, reader.read::<HarnessEvent>()).await
    {
        let done = matches!(event, HarnessEvent::AssistantEnded { .. });
        seen.push(event);
        if done {
            break;
        }
    }
    // Read before the mind drops, which takes the program with it.
    let asked = mind.asked() > 0;
    (seen, asked)
}

fn started_a_turn(seen: &[HarnessEvent]) -> bool {
    seen.iter()
        .any(|e| matches!(e, HarnessEvent::AssistantStarted { .. }))
}

fn was_committed(seen: &[HarnessEvent]) -> bool {
    seen.iter()
        .any(|e| matches!(e, HarnessEvent::MessageArrived { .. }))
}

#[tokio::test]
async fn an_answer_wakes_the_session_that_asked() {
    // The reported bug, end to end: two agents talked once and then stopped. `reply` sends an
    // `answer`, and an answer that does not start a turn means the asker never reads it.
    let (seen, asked) = arrival_of("answer", "answer").await;
    assert!(was_committed(&seen), "it never reached the transcript");
    assert!(
        started_a_turn(&seen) && asked,
        "the answer did not wake the session that asked: {seen:?}"
    );
}

#[tokio::test]
async fn a_question_wakes_the_session_it_was_put_to() {
    let (seen, asked) = arrival_of("question", "question").await;
    assert!(started_a_turn(&seen) && asked, "{seen:?}");
}

#[tokio::test]
async fn work_handed_over_wakes_whoever_it_was_handed_to() {
    // Otherwise the work stops with both sides believing the other has it.
    let (seen, asked) = arrival_of("handoff", "handoff").await;
    assert!(started_a_turn(&seen) && asked, "{seen:?}");
}

#[tokio::test]
async fn a_note_is_committed_without_starting_a_turn() {
    // The other half of the rule. A session that answers everything that arrives is one nobody
    // leaves running, and this is what stops the fix from being "wake for everything".
    let (seen, asked) = arrival_of("note", "note").await;
    assert!(
        was_committed(&seen),
        "a note still belongs in the transcript"
    );
    assert!(
        !started_a_turn(&seen) && !asked,
        "a note started a turn: {seen:?}"
    );
}
