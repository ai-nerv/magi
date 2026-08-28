//! The daemon, over a real socket.
//!
//! M1's claim is that `axum host` can stand in for the replay host without the UI changing.
//! These drive the same protocol the UI drives, so the claim is tested rather than asserted.

use axum_host::{open_session, serve};
use axum_ipc::{FrameReader, FrameWriter};
use axum_proto::{Cursor, Entry, HarnessEvent, UiCommand};
use std::path::{Path, PathBuf};

fn temp(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("axum-host-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    // Short enough to stay under SUN_LEN, which a scratch directory path is not.
    let socket = std::env::temp_dir().join(format!("axum-h-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    (dir, socket)
}

async fn start(name: &str) -> (PathBuf, PathBuf) {
    let (dir, socket) = temp(name);
    let session = open_session(&dir, "/tmp", 1).expect("session");
    let listener = axum_ipc::bind(&socket).await.expect("bind");
    tokio::spawn(async move { serve(listener, session, None).await });
    (dir, socket)
}

struct Client {
    reader: FrameReader<tokio::net::unix::OwnedReadHalf>,
    writer: FrameWriter<tokio::net::unix::OwnedWriteHalf>,
}

impl Client {
    async fn attach(socket: &Path, from: Cursor) -> (Self, Vec<Entry>) {
        let stream = axum_ipc::connect(socket).await.expect("connect");
        let (read_half, write_half) = stream.into_split();
        let mut client = Self {
            reader: FrameReader::new(read_half),
            writer: FrameWriter::new(write_half),
        };
        client
            .writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: from,
            })
            .await
            .expect("attach");
        match client
            .reader
            .read::<HarnessEvent>()
            .await
            .expect("snapshot")
        {
            HarnessEvent::SessionSnapshot { entries, .. } => (client, entries),
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    /// Attach and report which model the session says is answering.
    async fn model_of(socket: &Path) -> Option<axum_proto::ModelInfo> {
        let stream = axum_ipc::connect(socket).await.expect("connect");
        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);
        writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: Cursor::ZERO,
            })
            .await
            .expect("attach");
        match reader.read::<HarnessEvent>().await.expect("snapshot") {
            HarnessEvent::SessionSnapshot { model, .. } => model,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    async fn submit(&mut self, text: &str) {
        self.writer
            .write(&UiCommand::SubmitPrompt { text: text.into() })
            .await
            .expect("submit");
    }

    async fn next(&mut self) -> HarnessEvent {
        self.reader.read().await.expect("an event")
    }
}

#[tokio::test]
async fn a_submitted_prompt_comes_back_as_events() {
    let (dir, socket) = start("submit").await;
    let (mut client, entries) = Client::attach(&socket, Cursor::ZERO).await;
    assert!(entries.is_empty(), "a fresh session has no history");

    client.submit("hello").await;

    let first = client.next().await;
    match first {
        HarnessEvent::UserMessage { text, .. } => assert_eq!(text, "hello"),
        other => panic!("expected the user message first, got {other:?}"),
    }
    assert!(matches!(
        client.next().await,
        HarnessEvent::AssistantStarted { .. }
    ));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn a_prompt_survives_the_ui_and_is_replayed_on_reattach() {
    let (dir, socket) = start("reattach").await;
    {
        let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
        client.submit("remember me").await;
        // Drain until the turn settles, then drop the connection as a dying UI would.
        for _ in 0..4 {
            client.next().await;
        }
    }

    let (_client, entries) = Client::attach(&socket, Cursor(2)).await;
    assert_eq!(entries.len(), 2, "the snapshot carries what was committed");
    match &entries[0] {
        Entry::User { text, .. } => assert_eq!(text, "remember me"),
        other => panic!("expected a user entry, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn a_cold_reattach_replays_the_whole_session() {
    let (dir, socket) = start("cold").await;
    {
        let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
        client.submit("one").await;
        for _ in 0..4 {
            client.next().await;
        }
    }

    let (mut client, entries) = Client::attach(&socket, Cursor::ZERO).await;
    assert!(entries.is_empty(), "a cold attach carries no history");
    assert!(
        matches!(client.next().await, HarnessEvent::UserMessage { .. }),
        "it arrives as replayed events instead"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn two_uis_both_see_a_prompt_either_one_submits() {
    let (dir, socket) = start("two").await;
    let (mut a, _) = Client::attach(&socket, Cursor::ZERO).await;
    let (mut b, _) = Client::attach(&socket, Cursor::ZERO).await;

    a.submit("shared").await;

    for client in [&mut a, &mut b] {
        match client.next().await {
            HarnessEvent::UserMessage { text, .. } => assert_eq!(text, "shared"),
            other => panic!("both attached UIs see it, got {other:?}"),
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn the_journal_outlives_the_daemon() {
    let (dir, socket) = start("durable").await;
    {
        let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
        client.submit("persisted").await;
        for _ in 0..4 {
            client.next().await;
        }
    }

    let journal = std::fs::read_dir(&dir)
        .expect("read dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .expect("a journal");
    let source = std::fs::read_to_string(&journal).expect("read");
    assert!(source.contains("persisted"), "the prompt reached the disk");
    assert!(source.lines().count() >= 3, "meta, prompt, reply");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

/// A provider that accepts a request and never answers, leaving a turn in flight.
fn serve_silently() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for socket in listener.incoming() {
            held.push(socket);
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// A daemon with a real backend, so a submitted prompt starts a turn that does not end.
async fn start_with_backend(name: &str, base_url: String) -> (PathBuf, PathBuf) {
    use axum_provider::model::{Api, Modality, Model};
    use axum_provider::provider::{Auth, Provider};

    let (dir, socket) = temp(name);
    let session = open_session(&dir, "/tmp", 1).expect("session");
    let model = Model {
        id: "m".into(),
        name: "M".into(),
        provider: "fake".into(),
        api: Api::AnthropicMessages,
        reasoning: false,
        input: vec![Modality::Text],
        context_window: 200_000,
        max_tokens: 4096,
        cost: axum_model::Cost::default(),
        thinking: std::collections::BTreeMap::new(),
        compat: None,
    };
    let backend = axum_host::turn::Backend {
        apis: axum_lua::adapter::BUILTIN
            .iter()
            .map(|(n, s)| ((*n).to_owned(), (*s).to_owned()))
            .collect(),
        tools: Vec::new(),
        stubs: Vec::new(),
        cwd: std::env::temp_dir(),
        provider: Provider {
            id: "fake".into(),
            name: "Fake".into(),
            base_url: Some(base_url),
            api: Api::AnthropicMessages,
            auth: Auth::None,
            compat: None,
            models: vec![model.clone()],
        },
        model,
        options: axum_provider::api::Options::default(),
        system: Some("You are axum.".to_owned()),
        confine: false,
        grants: Vec::new(),
    };
    let listener = axum_ipc::bind(&socket).await.expect("bind");
    tokio::spawn(async move { serve(listener, session, Some(backend)).await });
    (dir, socket)
}

#[tokio::test]
async fn events_reach_the_ui_while_the_turn_is_still_running() {
    // The turn used to be awaited on the connection's own task, which is also the task that
    // forwards events. Nothing reached the screen until the turn was over, so a streaming
    // response arrived in one piece at the end and a slow one looked like a hang. Every other
    // test passed throughout, because they all use a provider that answers immediately.
    let (dir, socket) = start_with_backend("streaming", serve_silently()).await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
    client.submit("this will not be answered").await;

    let event = tokio::time::timeout(std::time::Duration::from_secs(5), client.next())
        .await
        .expect("the prompt is echoed before the turn ends, not after");
    assert!(
        matches!(event, HarnessEvent::UserMessage { ref text, .. } if text == "this will not be answered"),
        "{event:?}"
    );

    // And the status, which is what drives the spinner.
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), client.next())
        .await
        .expect("the working status arrives while the work is happening");
    assert!(
        matches!(
            event,
            HarnessEvent::AssistantStarted { .. } | HarnessEvent::StatusChanged { .. }
        ),
        "{event:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn an_interrupt_is_answered_while_a_turn_holds_the_connection() {
    // The interrupt has to be read by the same loop the turn used to block, so this fails the
    // same way the streaming test does if a turn ever goes back to being awaited inline.
    let (dir, socket) = start_with_backend("interrupt", serve_silently()).await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
    client.submit("this will be stopped").await;

    // Drain until the turn is under way, so the interrupt lands on a running turn.
    let mut started = false;
    while !started {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), client.next())
            .await
            .expect("events flow");
        started = matches!(event, HarnessEvent::AssistantStarted { .. });
    }

    client
        .writer
        .write(&UiCommand::Interrupt)
        .await
        .expect("interrupt");

    let mut aborted = false;
    for _ in 0..10 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), client.next())
            .await
            .expect("the interrupt is acted on");
        if matches!(
            event,
            HarnessEvent::AssistantEnded {
                stop_reason: axum_proto::StopReason::Aborted,
                ..
            }
        ) {
            aborted = true;
            break;
        }
    }
    assert!(aborted, "the turn ended as aborted");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn an_amended_entry_is_not_announced_as_a_new_one() {
    // A tool call is committed before it runs and amended with its result, and an assistant
    // message is committed empty and amended as it grows. Republishing the whole entry each
    // time told the UI a second call had started, so the transcript showed every tool twice
    // and every message once empty and once full.
    use axum_proto::{Entry, MessageId, ToolCallId, ToolResult};

    let (dir, socket) = temp("amend");
    let session = open_session(&dir, "/tmp", 1).expect("session");
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(session));

    let mut live = session.lock().await.subscribe();
    {
        let mut held = session.lock().await;
        held.commit(Entry::Tool {
            id: ToolCallId::new("c1"),
            name: "edit".into(),
            args: "{}".into(),
            result: None,
            thought_signature: None,
        })
        .expect("commit");
        held.amend(Entry::Tool {
            id: ToolCallId::new("c1"),
            name: "edit".into(),
            args: "{}".into(),
            result: Some(ToolResult {
                output: "done".into(),
                is_error: false,
            }),
            thought_signature: None,
        })
        .expect("amend");
    }

    let first = live.try_recv().expect("the call started");
    assert!(
        matches!(first, HarnessEvent::ToolCallStarted { .. }),
        "{first:?}"
    );
    let second = live.try_recv().expect("the call ended");
    assert!(
        matches!(second, HarnessEvent::ToolCallEnded { .. }),
        "{second:?}"
    );
    assert!(
        live.try_recv().is_err(),
        "one start and one end, nothing more"
    );

    // The same for a message that grows: the amendment carries what was added, not the whole
    // body, because a UI appending deltas would otherwise show the opening twice.
    {
        let mut held = session.lock().await;
        held.commit(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "Hello".into(),
            thinking: String::new(),
            stop_reason: None,
            error: None,
            signatures: axum_proto::Signatures::default(),
            usage: axum_proto::Usage::default(),
        })
        .expect("commit");
        held.amend(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "Hello there".into(),
            thinking: String::new(),
            stop_reason: Some(axum_proto::StopReason::EndTurn),
            error: None,
            signatures: axum_proto::Signatures::default(),
            usage: axum_proto::Usage::default(),
        })
        .expect("amend");
    }

    let _started = live.try_recv().expect("started");
    let _opening = live.try_recv().expect("the opening delta");
    let delta = live.try_recv().expect("the amendment's delta");
    assert!(
        matches!(delta, HarnessEvent::AssistantDelta { ref text, .. } if text == " there"),
        "{delta:?}"
    );
    let ended = live.try_recv().expect("ended");
    assert!(
        matches!(ended, HarnessEvent::AssistantEnded { .. }),
        "{ended:?}"
    );
    assert!(live.try_recv().is_err(), "nothing else was published");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn what_a_turn_cost_reaches_the_ui() {
    // The footer read zero against a real model while the journal held the real numbers. A
    // finished turn is published by `amendment_events` — the entry is committed empty and
    // amended once it has streamed — and that path was sending a default. `events_for`, which
    // only a cold replay uses, had the right one, so nothing that replayed noticed.
    use axum_proto::{Entry, MessageId, Signatures, StopReason, Usage};

    let (dir, socket) = temp("usage");
    let session = open_session(&dir, "/tmp", 1).expect("session");
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(session));
    let mut live = session.lock().await.subscribe();

    let spent = Usage {
        input: 124,
        output: 9,
        cache_read: 768,
        cache_write: 0,
    };
    {
        let mut held = session.lock().await;
        held.commit(Entry::Assistant {
            id: MessageId::new("a1"),
            text: String::new(),
            thinking: String::new(),
            stop_reason: None,
            error: None,
            signatures: Signatures::default(),
            usage: Usage::default(),
        })
        .expect("commit");
        held.amend(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "done".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
            signatures: Signatures::default(),
            usage: spent,
        })
        .expect("amend");
    }

    let mut reported = None;
    while let Ok(event) = live.try_recv() {
        if let HarnessEvent::AssistantEnded { usage, .. } = event {
            reported = Some(usage);
        }
    }
    assert_eq!(
        reported,
        Some(spent),
        "the cost is published, not defaulted"
    );

    // And the session's own total agrees, which is what a resumed footer reads.
    assert_eq!(session.lock().await.usage(), spent);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

/// A catalog with two reachable models and one that needs a key nobody has set.
fn two_models() -> axum_host::catalog::Catalog {
    let providers =
        serde_json::from_value::<Vec<axum_provider::provider::Provider>>(serde_json::json!([
            {
                "id": "local", "name": "Local", "api": "openai-completions",
                "base_url": "http://127.0.0.1:1/v1", "auth": { "kind": "none" },
                "models": [
                    { "id": "a", "name": "A", "context_window": 1000, "max_tokens": 100 },
                    { "id": "b", "name": "B", "context_window": 2000, "max_tokens": 100 }
                ]
            },
            {
                "id": "paid", "name": "Paid Co", "api": "openai-completions",
                "base_url": "https://paid.test/v1",
                "auth": { "kind": "api-key", "vars": ["AXUM_TEST_UNSET_KEY"] },
                "models": [
                    { "id": "x", "name": "X", "context_window": 1000, "max_tokens": 100 }
                ]
            }
        ]))
        .expect("providers");
    axum_host::catalog::Catalog {
        apis: axum_lua::adapter::BUILTIN
            .iter()
            .map(|(n, s)| ((*n).to_owned(), (*s).to_owned()))
            .collect(),
        tools: Vec::new(),
        stubs: Vec::new(),
        cwd: std::env::temp_dir(),
        providers,
        options: axum_provider::api::Options::default(),
        system: None,
        confine: false,
        grants: Vec::new(),
        chosen: None,
    }
}

async fn start_with_catalog(name: &str) -> (PathBuf, PathBuf) {
    let (dir, socket) = temp(name);
    let session = open_session(&dir, "/tmp", 1).expect("session");
    let catalog = two_models();
    let backend = catalog.backend("local/a");
    let listener = axum_ipc::bind(&socket).await.expect("bind");
    tokio::spawn(
        async move { axum_host::serve_catalog(listener, session, backend, catalog).await },
    );
    (dir, socket)
}

#[tokio::test]
async fn switching_model_is_announced_so_the_footer_can_follow() {
    // Republishing the status does not do it: a status event carries a status. A UI learns
    // which model is answering from the snapshot it attached with, and without an event of its
    // own there is nothing to change its mind — the switch worked and the footer lied.
    let (dir, socket) = start_with_catalog("switch").await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;

    client
        .writer
        .write(&UiCommand::SetModel {
            name: "local/b".to_owned(),
        })
        .await
        .expect("send");

    let mut announced = None;
    for _ in 0..10 {
        if let HarnessEvent::ModelChanged { model, .. } = client.next().await {
            announced = model;
            break;
        }
    }
    let model = announced.expect("the switch was announced");
    assert_eq!(model.name, "local/b");
    assert_eq!(model.context_window, 2000, "and its window came with it");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn a_model_that_does_not_exist_is_refused_with_the_ones_that_do() {
    let (dir, socket) = start_with_catalog("unknown").await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;

    client
        .writer
        .write(&UiCommand::SetModel {
            name: "nope/nope".to_owned(),
        })
        .await
        .expect("send");

    let HarnessEvent::Refused { message, .. } = client.next().await else {
        panic!("expected a refusal");
    };
    assert!(message.contains("no model called"), "{message}");
    assert!(
        message.contains("local/a"),
        "it lists what works: {message}"
    );
    // And not what does not: a list of models you cannot reach is one nobody reads.
    assert!(!message.contains("paid/x"), "{message}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn a_model_with_no_credential_is_refused_with_what_to_set() {
    // "No such model" and "you have not set a key" send a person to two different places.
    let (dir, socket) = start_with_catalog("uncredentialed").await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;

    client
        .writer
        .write(&UiCommand::SetModel {
            name: "paid/x".to_owned(),
        })
        .await
        .expect("send");

    let HarnessEvent::Refused { message, .. } = client.next().await else {
        panic!("expected a refusal");
    };
    assert!(message.contains("AXUM_TEST_UNSET_KEY"), "{message}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn a_refused_switch_leaves_the_session_answering_with_what_it_had() {
    // The new worker is built before the old one is dropped, so a switch that fails costs
    // nothing. A session that stopped working because a name was mistyped would be worse than
    // no `/model` at all.
    let (dir, socket) = start_with_catalog("kept").await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;

    client
        .writer
        .write(&UiCommand::SetModel {
            name: "nope".to_owned(),
        })
        .await
        .expect("send");
    let _refusal = client.next().await;

    // Asked of the session rather than inferred from a turn: what a refused switch must not
    // do is change which model answers, and that is a question with a direct answer.
    let model = Client::model_of(&socket).await.expect("still a model");
    assert_eq!(model.name, "local/a", "the switch was refused, not applied");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

/// A provider that keeps the request body it was sent, and then goes quiet.
fn serve_recording() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::Read;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let kept = std::sync::Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { continue };
            let mut buf = vec![0u8; 65_536];
            if let Ok(n) = socket.read(&mut buf) {
                kept.lock()
                    .expect("lock")
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
            held.push(socket);
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

#[tokio::test]
async fn the_model_is_told_what_it_is_before_the_conversation() {
    // Every adapter reads `ctx.system` and has since the first milestone. Nothing ever set it,
    // so six milestones of turns went out with tool schemas and no idea what the model was,
    // where it was, or what machine it was on. Five hundred tests passed throughout.
    let (base_url, seen) = serve_recording();
    let (dir, socket) = start_with_backend("system-prompt", base_url).await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
    client.submit("hello").await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let body = loop {
        if let Some(request) = seen.lock().expect("lock").first().cloned() {
            break request;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the provider was never called"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    assert!(
        body.contains("You are axum."),
        "the system prompt has to reach the wire, not just the struct: {body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}
