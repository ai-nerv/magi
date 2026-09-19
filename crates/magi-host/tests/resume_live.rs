use magi_host::{catalog::Catalog, scribe::Scribe, session::Session, turn::Backend};
use magi_ipc::{FrameReader, FrameWriter, family::Family};
use magi_model::scratch::Scratch;
use magi_proto::{
    AgentStatus, Cursor, Entry, HarnessEvent, MessageId, SessionId, StopReason, UiCommand,
};
use magi_testkit::{Mind, memory::Serving};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

fn user(id: &str, text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(id),
        text: text.into(),
        aside: String::new(),
    }
}

fn answer(id: &str, text: &str) -> Entry {
    Entry::Assistant {
        id: MessageId::new(id),
        text: text.into(),
        thinking: String::new(),
        stop_reason: Some(StopReason::EndTurn),
        error: None,
        signatures: Default::default(),
        usage: Default::default(),
    }
}

async fn scribe(memory: &Serving, id: &str) -> Scribe {
    let family = Family::dial(memory.socket())
        .await
        .expect("dial disposable memory");
    Scribe::over(
        family,
        Some(memory.socket().to_owned()),
        &SessionId::new(id),
    )
}

async fn event(
    reader: &mut FrameReader<OwnedReadHalf>,
    matches: impl Fn(&HarnessEvent) -> bool,
) -> HarnessEvent {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let event = reader.read::<HarnessEvent>().await.expect("host event");
            if matches(&event) {
                return event;
            }
        }
    })
    .await
    .expect("event barrier")
}

async fn attach(
    path: &std::path::Path,
) -> (FrameReader<OwnedReadHalf>, FrameWriter<OwnedWriteHalf>) {
    let (read, write) = magi_ipc::connect(path)
        .await
        .expect("connect host")
        .into_split();
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor(0),
            draws: false,
        })
        .await
        .expect("attach");
    (FrameReader::new(read), writer)
}

struct Host(tokio::task::JoinHandle<Result<(), magi_host::HostError>>);
impl Drop for Host {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn in_process_resume_writes_only_to_the_selected_transcript() {
    resume_binding("binding", "B", [1, 2], false).await;
}

#[tokio::test]
async fn in_process_resume_preserves_sparse_cursors_and_child_ownership() {
    resume_binding("sparse", "B@child", [7, 13], false).await;
}

#[tokio::test]
async fn resumed_lua_tools_and_watchers_use_the_selected_session() {
    resume_binding("identity", "B", [1, 2], true).await;
}

async fn resume_binding(name: &str, target: &str, cursors: [u64; 2], identity: bool) {
    let dir = Scratch::new("magi-resume", name);
    let Some(memory) = Serving::start(&dir, &format!("resume-{name}")).await else {
        return;
    };
    let original_a = vec![user("a-u1", "only A"), answer("a-a2", "answer A")];
    let original_b = vec![user("b-u1", "only B"), answer("b-a2", "answer B")];
    for (id, entries) in [("A", &original_a), ("B", &original_b)] {
        let mut scribe = scribe(&memory, id)
            .await
            .recording_as(if id == "B" { target } else { id }.into());
        for (n, entry) in entries.iter().enumerate() {
            scribe
                .observe(
                    Cursor(if id == "B" { cursors[n] } else { n as u64 + 1 }),
                    entry,
                    &Default::default(),
                )
                .await
                .expect("seed transcript");
        }
    }
    let mind = if identity {
        let call = magi_testkit::mind::call_lines("identity", "identity", "{}");
        Mind::turns(
            &format!("resume-{name}"),
            &[
                &call.iter().map(String::as_str).collect::<Vec<_>>(),
                &[
                    &magi_testkit::mind::text_line("continued B"),
                    &magi_testkit::mind::stop_line(),
                ],
            ],
        )
    } else {
        Mind::answering(&format!("resume-{name}"), "continued B")
    };
    let backend = Backend {
        tools: if identity {
            vec![(
                "identity".into(),
                r#"
            local seen = {}
            magi.watch("identity", { run = function(event)
                if event.kind == "session.opened" then seen[#seen + 1] = event.id end
            end })
            magi.tool("identity", {
                description = "Current session", parameters = {type = "object"},
                transport = {kind = "lua"}, run = function()
                    return magi.session .. ":" .. table.concat(seen, ",")
                end,
            })
        "#
                .into(),
            )]
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
        deciders: Vec::new(),
    };
    let path = dir.join("host.sock");
    let host = Host(tokio::spawn(magi_host::serve_on(
        magi_ipc::bind(&path).await.expect("bind"),
        Session::recorded(SessionId::new("A"), original_a.clone()),
        Some(backend),
        Catalog::empty(),
        Some(memory.socket().to_owned()),
    )));
    let (mut first, mut writer) = attach(&path).await;
    let (mut second, _other) = attach(&path).await;
    event(&mut first, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    event(&mut second, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    if identity {
        writer
            .write(&UiCommand::TakeGrants { grants: Vec::new() })
            .await
            .expect("warm A worker");
        event(&mut first, |e| {
            matches!(
                e,
                HarnessEvent::StatusChanged {
                    status: AgentStatus::Idle,
                    ..
                }
            )
        })
        .await;
    }
    writer
        .write(&UiCommand::Resume {
            id: "missing-transcript".into(),
        })
        .await
        .expect("missing resume");
    event(&mut first, |e| matches!(e, HarnessEvent::Refused {message,..} if message.contains("nonempty transcript"))).await;
    let (mut unchanged, _writer) = attach(&path).await;
    let unchanged = event(&mut unchanged, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    assert!(
        matches!(unchanged, HarnessEvent::SessionSnapshot {session,..} if session.as_str() == "A")
    );
    writer
        .write(&UiCommand::Resume { id: target.into() })
        .await
        .expect("resume B");
    for reader in [&mut first, &mut second] {
        let snapshot = event(reader, |e| matches!(e, HarnessEvent::SessionSnapshot { session, .. } if session.as_str() == "B")).await;
        assert!(
            matches!(snapshot, HarnessEvent::SessionSnapshot { entries, .. } if entries == original_b)
        );
    }
    writer
        .write(&UiCommand::SubmitPrompt {
            text: "new B prompt".into(),
            aside: String::new(),
        })
        .await
        .expect("continue B");
    event(&mut first, |e| {
        matches!(e, HarnessEvent::AssistantEnded { .. })
    })
    .await;
    event(&mut first, |e| {
        matches!(
            e,
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
    let actual_a = scribe(&memory, "A")
        .await
        .replay()
        .await
        .expect("independent A replay");
    let actual_b = scribe(&memory, "B")
        .await
        .recording_as(target.into())
        .replay()
        .await
        .expect("independent B replay");
    drop(host);
    assert_eq!(
        actual_a.iter().map(|(_, e)| e).collect::<Vec<_>>(),
        original_a.iter().collect::<Vec<_>>(),
        "A changed after selecting B"
    );
    let added = if identity { 4 } else { 2 };
    assert_eq!(
        actual_b.len(),
        2 + added,
        "B did not receive the continuation"
    );
    assert_eq!(
        actual_b.iter().map(|(c, _)| c.0).collect::<Vec<_>>(),
        [cursors[0], cursors[1]]
            .into_iter()
            .chain((1..=added).map(|n| cursors[1] + n as u64))
            .collect::<Vec<_>>()
    );
    assert_eq!(actual_b[0].1, original_b[0]);
    assert_eq!(actual_b[1].1, original_b[1]);
    assert!(matches!(&actual_b[2].1, Entry::User { text, .. } if text == "new B prompt"));
    assert!(
        matches!(&actual_b.last().expect("answer").1, Entry::Assistant { text, .. } if text == "continued B")
    );
    if identity {
        assert!(
            matches!(&actual_b[4].1, Entry::Tool {result:Some(result),..} if result.output == "B:A,B" && !result.is_error)
        );
    }
    assert!(mind.asks()[0].contains("only B"));
    assert!(!mind.asks()[0].contains("only A"));
    drop(memory);
    let reopened = Serving::start(&dir, &format!("resume-{name}"))
        .await
        .expect("reopen store");
    assert_eq!(
        scribe(&reopened, "A")
            .await
            .replay()
            .await
            .expect("reopened A"),
        actual_a
    );
    assert_eq!(
        scribe(&reopened, "B")
            .await
            .recording_as(target.into())
            .replay()
            .await
            .expect("reopened B"),
        actual_b
    );
}
