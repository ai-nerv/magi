//! What happens when another session says something, against a real session on a real socket.
//!
//! The rule under test is invisible when it is wrong. An arrival is committed either way, so a
//! session that should have answered and did not looks exactly like one with nothing to say:
//! the message is in the transcript, the screen is idle, and nothing anywhere reports a problem.
//!
//! That is how `ask` came to be a one-way trip. It sends a `question`, which woke the receiver;
//! `reply` sends an `answer`, which woke nobody — so two agents got one exchange and stopped.

use axon_host::session::Session;
use axon_host::turn::Backend;
use axon_ipc::{FrameReader, FrameWriter};
use axon_proto::{Cursor, HarnessEvent, SessionId, UiCommand};
use axon_provider::api::Options;
use axon_provider::model::{Api, Modality, Model};
use axon_provider::provider::{Auth, Provider};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

/// A recorded Anthropic response: a little text, then a clean stop.
const STREAM: &str = "\
event: message_start\n\
data: {\"message\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":0}}}\n\
\n\
event: content_block_start\n\
data: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\
\n\
event: content_block_delta\n\
data: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"thanks\"}}\n\
\n\
event: message_delta\n\
data: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\
\n";

/// Answer one request with `STREAM`, and report whether anything ever asked.
///
/// The flag is the real assertion in the negative case: "no turn ran" has to mean the provider
/// was never called, not merely that no event happened to arrive before a timeout.
fn serve_once() -> (String, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&asked);
    std::thread::spawn(move || {
        let Ok((mut socket, _)) = listener.accept() else {
            return;
        };
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut reader = BufReader::new(socket.try_clone().expect("clone"));
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }
        let _ = write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{STREAM}",
            STREAM.len()
        );
        let _ = socket.flush();
    });
    (format!("http://127.0.0.1:{port}"), asked)
}

fn backend(base_url: String) -> Backend {
    let model = Model {
        id: "claude-sonnet-4-5".into(),
        name: "Claude Sonnet 4.5".into(),
        provider: "fake".into(),
        api: Api::AnthropicMessages,
        reasoning: false,
        input: vec![Modality::Text],
        context_window: 200_000,
        max_tokens: 8192,
        cost: axon_model::Cost::default(),
        thinking: BTreeMap::new(),
        compat: None,
    };
    Backend {
        // The real ones, because the worker builds its own adapter out of them: a backend with
        // no protocol description cannot answer at all, and the failure would read as "the
        // arrival did not wake it".
        apis: axon_lua::adapter::shipped_apis().expect("the shipped protocols"),
        tools: Vec::new(),
        clients: Vec::new(),
        cwd: std::env::temp_dir(),
        provider: Provider {
            id: "fake".into(),
            name: "Fake".into(),
            base_url: Some(base_url),
            api: Api::AnthropicMessages,
            auth: Auth::None,
            compat: None,
            models: vec![model.clone()],
            discover: false,
        },
        model,
        options: Options::default(),
        system: None,
        confine: false,
        grants: Vec::new(),
        environ: BTreeMap::new(),
    }
}

/// A session serving on its own socket, and the path to reach it at.
async fn serving(name: &str, base_url: String) -> std::path::PathBuf {
    // Short: a scratch directory path does not fit in `SUN_LEN`.
    let path = std::env::temp_dir().join(format!("axon-arr-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let dir = std::env::temp_dir().join(format!("axon-arr-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let session = Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("s");
    let listener = axon_ipc::bind(&path).await.expect("bind");
    let backend = backend(base_url);
    tokio::spawn(async move {
        let _ = axon_host::serve_catalog(
            listener,
            session,
            Some(backend),
            axon_host::catalog::Catalog::empty(),
        )
        .await;
    });
    path
}

/// Attach, hand over one arrival, and report every event that follows within `patience`.
async fn arrival_of(sort: &str, name: &str) -> (Vec<HarnessEvent>, bool) {
    let (base_url, asked) = serve_once();
    let path = serving(name, base_url).await;
    let stream = axon_ipc::connect(&path).await.expect("connect");
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor(0),
        })
        .await
        .expect("attach");
    let _snapshot: HarnessEvent = reader.read().await.expect("a snapshot");

    writer
        .write(&UiCommand::Arrived {
            who: "axon/main/beta-nu".to_owned(),
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
    (seen, asked.load(std::sync::atomic::Ordering::SeqCst))
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
