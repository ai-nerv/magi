use magi_host::{catalog::Catalog, session::Session, turn::Backend};
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, Entry, HarnessEvent, SessionId, UiCommand};
use magi_testkit::Mind;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

async fn attach(path: &std::path::Path, cursor: Cursor) -> (Reader, Writer) {
    let stream = magi_ipc::connect(path).await.expect("connect");
    let (read, write) = stream.into_split();
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: cursor,
            draws: false,
        })
        .await
        .expect("attach");
    (FrameReader::new(read), writer)
}

async fn until(reader: &mut Reader, matches: impl Fn(&HarnessEvent) -> bool) -> HarnessEvent {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let event: HarnessEvent = reader.read().await.expect("event");
            if matches(&event) {
                return event;
            }
        }
    })
    .await
    .expect("event barrier")
}

struct Fixture {
    dir: Scratch,
    serving: tokio::task::JoinHandle<Result<(), magi_host::HostError>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

async fn serving(name: &str, mind: &Mind) -> Fixture {
    configured(name, mind, false).await
}

async fn configured(name: &str, mind: &Mind, broken: bool) -> Fixture {
    let dir = Scratch::new("magi-life", name);
    let path = dir.join("s.sock");
    let listener = magi_ipc::bind(&path).await.expect("bind");
    let backend = Backend {
        tools: if broken {
            vec![("broken".into(), "not lua at all".into())]
        } else {
            Vec::new()
        },
        clients: Vec::new(),
        tooling: Default::default(),
        cwd: dir.to_path_buf(),
        model: "fake/one".into(),
        mind: mind.program().display().to_string(),
        wants: Default::default(),
        context_window: Some(200_000),
        max_output: None,
        system: None,
        confine: false,
        isolate: false,
        grants: Vec::new(),
        environ: Default::default(),
        helpers: Default::default(),
    };
    let serving = tokio::spawn(magi_host::serve_on(
        listener,
        Session::recorded(SessionId::new(name), Vec::new()),
        Some(backend),
        Catalog::empty(),
        Some(dir.join("absent-memory.sock")),
    ));
    Fixture { dir, serving }
}

async fn prompt(writer: &mut Writer, text: &str) {
    writer
        .write(&UiCommand::SubmitPrompt {
            text: text.into(),
            aside: String::new(),
        })
        .await
        .expect("command sent");
}

async fn barrier(reader: &mut Reader, writer: &mut Writer) {
    writer
        .write(&UiCommand::SetThinking {
            level: "invalid-barrier".into(),
        })
        .await
        .expect("command sent");
    until(reader, |e| matches!(e, HarnessEvent::Refused { .. })).await;
}

async fn prefix(reader: &mut Reader, nth: usize) {
    let wanted = format!("part-{nth}");
    until(
        reader,
        |e| matches!(e, HarnessEvent::AssistantDelta { text, .. } if text == &wanted),
    )
    .await;
}

async fn snapshot(path: &std::path::Path) -> Vec<Entry> {
    let (mut reader, _writer) = attach(path, Cursor(u64::MAX)).await;
    match until(&mut reader, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await
    {
        HarnessEvent::SessionSnapshot { entries, .. } => entries,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn two_clients_keep_each_prompt_and_answer_in_order() {
    let mind = Mind::controlled("lifecycle-fifo");
    let fixture = serving("fifo", &mind).await;
    let path = fixture.dir.join("s.sock");
    let (mut first, mut send_first) = attach(&path, Cursor(0)).await;
    let (mut second, mut send_second) = attach(&path, Cursor(0)).await;
    send_first
        .write(&UiCommand::SubmitPrompt {
            text: "first prompt".into(),
            aside: String::new(),
        })
        .await
        .expect("command sent");
    until(
        &mut first,
        |e| matches!(e, HarnessEvent::AssistantDelta { text, .. } if text == "part-0"),
    )
    .await;
    send_second
        .write(&UiCommand::SubmitPrompt {
            text: "second prompt".into(),
            aside: String::new(),
        })
        .await
        .expect("command sent");
    send_second
        .write(&UiCommand::SetThinking {
            level: "invalid-barrier".into(),
        })
        .await
        .expect("command sent");
    until(&mut second, |e| matches!(e, HarnessEvent::Refused { .. })).await;
    mind.release(0);
    until(
        &mut first,
        |e| matches!(e, HarnessEvent::AssistantDelta { text, .. } if text == "part-1"),
    )
    .await;
    mind.release(1);
    until(&mut first, |e| {
        matches!(e, HarnessEvent::AssistantEnded { .. })
    })
    .await;
    let (mut snapshot, _sender) = attach(&path, Cursor(u64::MAX)).await;
    let HarnessEvent::SessionSnapshot { entries, .. } = until(&mut snapshot, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert!(!mind.overlapped(), "provider requests overlapped");
    let bodies: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::User { text, .. } | Entry::Assistant { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        bodies,
        [
            "first prompt",
            "part-0-done",
            "second prompt",
            "part-1-done"
        ]
    );
    assert_eq!(mind.asked(), 2);
    assert!(mind.asks()[1].contains("second prompt"));
}

#[tokio::test]
async fn interrupt_does_not_cancel_the_queued_replacement() {
    let mind = Mind::controlled("lifecycle-interrupt");
    let fixture = serving("interrupt", &mind).await;
    let path = fixture.dir.join("s.sock");
    let (mut reader, mut writer) = attach(&path, Cursor(0)).await;
    prompt(&mut writer, "interrupted").await;
    prefix(&mut reader, 0).await;
    writer
        .write(&UiCommand::Interrupt)
        .await
        .expect("command sent");
    prompt(&mut writer, "replacement").await;
    prefix(&mut reader, 1).await;
    mind.release(1);
    until(&mut reader, |e| {
        matches!(
            e,
            HarnessEvent::StatusChanged {
                status: magi_proto::AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
    let entries = snapshot(&path).await;
    assert!(
        matches!(&entries[1], Entry::Assistant { text, stop_reason: Some(magi_proto::StopReason::Aborted), .. } if text == "part-0")
    );
    assert!(matches!(&entries[2], Entry::User { text, .. } if text == "replacement"));
    assert!(
        matches!(&entries[3], Entry::Assistant { text, stop_reason: Some(magi_proto::StopReason::EndTurn), .. } if text == "part-1-done")
    );
    assert_eq!(entries.len(), 4);
    assert_eq!(mind.asked(), 2);
    assert!(!mind.overlapped());
}

#[tokio::test]
async fn busy_reconfiguration_is_refused_and_disconnect_keeps_accepted_work() {
    let mind = Mind::controlled("lifecycle-disconnect");
    let fixture = serving("disconnect", &mind).await;
    let path = fixture.dir.join("s.sock");
    let (mut reader, mut writer) = attach(&path, Cursor(0)).await;
    prompt(&mut writer, "original").await;
    prefix(&mut reader, 0).await;
    for command in [
        UiCommand::SetModel {
            name: "fake/one".into(),
        },
        UiCommand::SetProvider {
            provider: Some("other".into()),
        },
        UiCommand::SetThinking {
            level: "high".into(),
        },
        UiCommand::Resume { id: "other".into() },
        UiCommand::Branch { keeps: Some(0) },
    ] {
        writer.write(&command).await.expect("command sent");
        let refusal = until(&mut reader, |e| matches!(e, HarnessEvent::Refused { .. })).await;
        assert!(
            matches!(refusal, HarnessEvent::Refused { message, .. } if message.contains("busy"))
        );
    }
    writer
        .write(&UiCommand::TakeGrants { grants: Vec::new() })
        .await
        .expect("command sent");
    prompt(&mut writer, "survives disconnect").await;
    barrier(&mut reader, &mut writer).await;
    drop(reader);
    drop(writer);
    let (mut reader, _writer) = attach(&path, Cursor(0)).await;
    mind.release(0);
    prefix(&mut reader, 1).await;
    mind.release(1);
    until(
        &mut reader,
        |e| matches!(e, HarnessEvent::AssistantEnded { id, .. } if id.as_str() == "a4"),
    )
    .await;
    let entries = snapshot(&path).await;
    assert_eq!(entries.len(), 4);
    assert!(matches!(&entries[2], Entry::User { text, .. } if text == "survives disconnect"));
    assert!(matches!(&entries[3], Entry::Assistant { text, .. } if text == "part-1-done"));
    assert_eq!(mind.asked(), 2);
    assert!(!mind.overlapped());
}

#[tokio::test]
async fn a_stopped_worker_reports_each_accepted_prompt_instead_of_stranding_the_queue() {
    let mind = Mind::controlled("lifecycle-stopped");
    let fixture = configured("stopped", &mind, true).await;
    let path = fixture.dir.join("s.sock");
    let (mut reader, mut writer) = attach(&path, Cursor(0)).await;
    prompt(&mut writer, "first failed").await;
    prompt(&mut writer, "second failed").await;
    until(
        &mut reader,
        |e| matches!(e, HarnessEvent::AssistantEnded { id, .. } if id.as_str() == "a4"),
    )
    .await;
    let entries = snapshot(&path).await;
    assert_eq!(entries.len(), 4);
    for (n, text) in ["first failed", "second failed"].into_iter().enumerate() {
        assert!(matches!(&entries[n * 2], Entry::User { text: actual, .. } if actual == text));
        assert!(
            matches!(&entries[n * 2 + 1], Entry::Assistant { error: Some(why), .. } if why.contains("worker stopped"))
        );
    }
    assert_eq!(mind.asked(), 0);
}

#[tokio::test]
async fn arrivals_and_declarations_share_the_prompt_boundary() {
    let mind = Mind::controlled("lifecycle-arrivals");
    let fixture = serving("arrivals", &mind).await;
    let path = fixture.dir.join("s.sock");
    let (mut reader, mut writer) = attach(&path, Cursor(0)).await;
    prompt(&mut writer, "original").await;
    prefix(&mut reader, 0).await;
    for (sort, text) in [("note", "note first"), ("question", "question next")] {
        writer
            .write(&UiCommand::Arrived {
                who: "peer".into(),
                kin: "main".into(),
                sort: sort.into(),
                text: text.into(),
            })
            .await
            .expect("command sent");
    }
    writer
        .write(&UiCommand::DeclareNeeds)
        .await
        .expect("command sent");
    prompt(&mut writer, "last prompt").await;
    barrier(&mut reader, &mut writer).await;
    assert_eq!(mind.asked(), 1);
    mind.release(0);
    prefix(&mut reader, 1).await;
    mind.release(1);
    let (mut observing, mut control) = attach(&path, Cursor(0)).await;
    until(
        &mut observing,
        |e| matches!(e, HarnessEvent::AssistantEnded { id, .. } if id.as_str() == "a5"),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while mind.asked() < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("declaration request started");
    control
        .write(&UiCommand::Interrupt)
        .await
        .expect("command sent");
    prefix(&mut reader, 3).await;
    mind.release(3);
    until(
        &mut reader,
        |e| matches!(e, HarnessEvent::AssistantEnded { id, .. } if id.as_str() == "a7"),
    )
    .await;
    let entries = snapshot(&path).await;
    assert_eq!(entries.len(), 7);
    assert!(matches!(&entries[2], Entry::From { text, .. } if text == "note first"));
    assert!(matches!(&entries[3], Entry::From { text, .. } if text == "question next"));
    assert!(matches!(&entries[5], Entry::User { text, .. } if text == "last prompt"));
    assert!(matches!(&entries[6], Entry::Assistant { text, .. } if text == "part-3-done"));
    assert!(!mind.overlapped());
    assert_eq!(mind.asked(), 4);
}
