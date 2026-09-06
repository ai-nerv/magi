//! The daemon, over a real socket.
//!
//! M1's claim is that `magi host` can stand in for the replay host without the UI changing.
//! These drive the same protocol the UI drives, so the claim is tested rather than asserted.

use magi_model::scratch::Scratch;

use magi_host::{open_session, serve};
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, Entry, HarnessEvent, UiCommand};
use magi_testkit::Mind;
use std::path::{Path, PathBuf};

fn temp(name: &str) -> (Scratch, PathBuf) {
    let dir = Scratch::new("magi-host", name);
    // Inside the scratch, so the guard takes it with the rest. Still well under SUN_LEN: the
    // whole path is the temporary directory, one short name and `s.sock`.
    let socket = dir.join("s.sock");
    (dir, socket)
}

async fn start(name: &str) -> (Scratch, PathBuf) {
    let (dir, socket) = temp(name);
    let session = open_session(&dir, "/tmp", 1, "").expect("session");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    tokio::spawn(async move { serve(listener, session, None).await });
    (dir, socket)
}

struct Client {
    reader: FrameReader<tokio::net::unix::OwnedReadHalf>,
    writer: FrameWriter<tokio::net::unix::OwnedWriteHalf>,
}

impl Client {
    async fn attach(socket: &Path, from: Cursor) -> (Self, Vec<Entry>) {
        let stream = magi_ipc::connect(socket).await.expect("connect");
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
                draws: false,
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
    async fn model_of(socket: &Path) -> Option<magi_proto::ModelInfo> {
        let stream = magi_ipc::connect(socket).await.expect("connect");
        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);
        writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: Cursor::ZERO,
                draws: false,
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
            .write(&UiCommand::SubmitPrompt {
                text: text.into(),
                aside: String::new(),
            })
            .await
            .expect("submit");
    }

    async fn next(&mut self) -> HarnessEvent {
        self.reader.read().await.expect("an event")
    }
}

#[tokio::test]
async fn a_submitted_prompt_comes_back_as_events() {
    let (_dir, socket) = start("submit").await;
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
}

#[tokio::test]
async fn a_prompt_survives_the_ui_and_is_replayed_on_reattach() {
    let (_dir, socket) = start("reattach").await;
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
}

#[tokio::test]
async fn a_cold_reattach_replays_the_whole_session() {
    let (_dir, socket) = start("cold").await;
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
}

#[tokio::test]
async fn two_uis_both_see_a_prompt_either_one_submits() {
    let (_dir, socket) = start("two").await;
    let (mut a, _) = Client::attach(&socket, Cursor::ZERO).await;
    let (mut b, _) = Client::attach(&socket, Cursor::ZERO).await;

    a.submit("shared").await;

    for client in [&mut a, &mut b] {
        match client.next().await {
            HarnessEvent::UserMessage { text, .. } => assert_eq!(text, "shared"),
            other => panic!("both attached UIs see it, got {other:?}"),
        }
    }
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
}

/// A daemon that asks `mind`, so a submitted prompt starts a real turn.
async fn start_with_mind(name: &str, mind: &Mind) -> (Scratch, PathBuf) {
    let (dir, socket) = temp(name);
    let session = open_session(&dir, "/tmp", 1, "").expect("session");
    let backend = magi_host::turn::Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        casper: None,
        cwd: std::env::temp_dir(),
        model: "fake/one".to_owned(),
        mind: mind.program().display().to_string(),
        wants: magi_proto::ask::Wants::default(),
        context_window: Some(200_000),
        system: Some("You are magi.".to_owned()),
        confine: false,
        grants: Vec::new(),
        environ: std::collections::BTreeMap::new(),
    };
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    tokio::spawn(async move { serve(listener, session, Some(backend)).await });
    (dir, socket)
}

#[tokio::test]
async fn events_reach_the_ui_while_the_turn_is_still_running() {
    // The turn used to be awaited on the connection's own task, which is also the task that
    // forwards events. Nothing reached the screen until the turn was over, so a streaming
    // response arrived in one piece at the end and a slow one looked like a hang. Every other
    // test passed throughout, because they all use a provider that answers immediately.
    let mind = Mind::silent("rt-streaming");
    let (_dir, socket) = start_with_mind("streaming", &mind).await;
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
}

#[tokio::test]
async fn an_interrupt_is_answered_while_a_turn_holds_the_connection() {
    // The interrupt has to be read by the same loop the turn used to block, so this fails the
    // same way the streaming test does if a turn ever goes back to being awaited inline.
    let mind = Mind::silent("rt-interrupt");
    let (_dir, socket) = start_with_mind("interrupt", &mind).await;
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
                stop_reason: magi_proto::StopReason::Aborted,
                ..
            }
        ) {
            aborted = true;
            break;
        }
    }
    assert!(aborted, "the turn ended as aborted");
}

#[tokio::test]
async fn an_amended_entry_is_not_announced_as_a_new_one() {
    // A tool call is committed before it runs and amended with its result, and an assistant
    // message is committed empty and amended as it grows. Republishing the whole entry each
    // time told the UI a second call had started, so the transcript showed every tool twice
    // and every message once empty and once full.
    use magi_proto::{Entry, MessageId, ToolCallId, ToolResult};

    let (dir, _socket) = temp("amend");
    let session = open_session(&dir, "/tmp", 1, "").expect("session");
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
                shown: None,
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
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        })
        .expect("commit");
        held.amend(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "Hello there".into(),
            thinking: String::new(),
            stop_reason: Some(magi_proto::StopReason::EndTurn),
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
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
}

#[tokio::test]
async fn what_a_turn_cost_reaches_the_ui() {
    // The footer read zero against a real model while the journal held the real numbers. A
    // finished turn is published by `amendment_events` — the entry is committed empty and
    // amended once it has streamed — and that path was sending a default. `events_for`, which
    // only a cold replay uses, had the right one, so nothing that replayed noticed.
    use magi_proto::{Entry, MessageId, Signatures, StopReason, Usage};

    let (dir, _socket) = temp("usage");
    let session = open_session(&dir, "/tmp", 1, "").expect("session");
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
}

/// A catalog with two reachable models and one that needs a key nobody has set.
///
/// Cards, because that is what melchior sends: a name, a window and whether it is ready. Where
/// the model lives and what credential it takes are melchior's, and this is the whole of what
/// magi is given to pick from.
fn two_models() -> magi_host::catalog::Catalog {
    let cards = serde_json::from_value::<Vec<magi_proto::ask::Card>>(serde_json::json!([
        {
            "id": "local/a", "provider": "local", "name": "a",
            "api": "openai-completions", "context_window": 1000,
            "max_output": 100, "reasons": false, "ready": true
        },
        {
            "id": "local/b", "provider": "local", "name": "b",
            "api": "openai-completions", "context_window": 2000,
            "max_output": 100, "reasons": false, "ready": true
        },
        {
            "id": "paid/x", "provider": "paid", "name": "x",
            "api": "openai-completions", "context_window": 1000,
            "max_output": 100, "reasons": false, "ready": false,
            "needs": "MAGI_TEST_UNSET_KEY"
        }
    ]))
    .expect("cards");
    magi_host::catalog::Catalog {
        cards,
        ..magi_host::catalog::Catalog::empty()
    }
}

async fn start_with_catalog(name: &str) -> (Scratch, PathBuf) {
    let (dir, socket) = temp(name);
    let session = open_session(&dir, "/tmp", 1, "").expect("session");
    let catalog = two_models();
    let backend = catalog.backend("local/a");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    tokio::spawn(
        async move { magi_host::serve_catalog(listener, session, backend, catalog).await },
    );
    (dir, socket)
}

#[tokio::test]
async fn switching_model_is_announced_so_the_footer_can_follow() {
    // Republishing the status does not do it: a status event carries a status. A UI learns
    // which model is answering from the snapshot it attached with, and without an event of its
    // own there is nothing to change its mind — the switch worked and the footer lied.
    let (_dir, socket) = start_with_catalog("switch").await;
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
}

#[tokio::test]
async fn a_model_that_does_not_exist_is_refused_with_the_ones_that_do() {
    let (_dir, socket) = start_with_catalog("unknown").await;
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
}

#[tokio::test]
async fn a_model_with_no_credential_is_refused_with_what_to_set() {
    // "No such model" and "you have not set a key" send a person to two different places.
    let (_dir, socket) = start_with_catalog("uncredentialed").await;
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
    assert!(message.contains("MAGI_TEST_UNSET_KEY"), "{message}");
}

#[tokio::test]
async fn a_refused_switch_leaves_the_session_answering_with_what_it_had() {
    // The new worker is built before the old one is dropped, so a switch that fails costs
    // nothing. A session that stopped working because a name was mistyped would be worse than
    // no `/model` at all.
    let (_dir, socket) = start_with_catalog("kept").await;
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
}

#[tokio::test]
async fn the_model_is_told_what_it_is_before_the_conversation() {
    // Every adapter reads the system prompt and has since the first milestone. Nothing ever
    // set it, so six milestones of turns went out with tool schemas and no idea what the model
    // was, where it was, or what machine it was on. Five hundred tests passed throughout.
    //
    // Still worth its own test now that melchior does the sending: what magi has to get right
    // is putting it in the ask, and a prompt that reached the struct and not the pipe looks
    // exactly like one that worked.
    let mind = Mind::answering("rt-system", "hello yourself");
    let (_dir, socket) = start_with_mind("system-prompt", &mind).await;
    let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
    client.submit("hello").await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let ask = loop {
        let heard = mind.heard();
        if !heard.is_empty() {
            break heard;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the mind was never asked"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    assert!(
        ask.contains("You are magi."),
        "the system prompt has to reach the pipe, not just the struct: {ask}"
    );
    assert!(ask.contains("hello"), "and the conversation with it: {ask}");
}
