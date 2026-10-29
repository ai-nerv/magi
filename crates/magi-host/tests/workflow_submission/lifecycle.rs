use super::*;
use magi_proto::{Cursor, MessageId};

fn source(body: &str) -> String {
    format!("---\nname: review\ndescription: Lifecycle review\n---\n{body}\n")
}

fn install(fixture: &Fixture, body: &str) {
    put(
        &fixture.dir.join(".magi/skills/review/SKILL.md"),
        &source(body),
    );
}

async fn gated(name: &str) -> Option<Fixture> {
    let fixture = Fixture::configured(name, false, |catalog| {
        let quote = |path: &Path| magi_proto::permit::quoted(path.to_str().expect("fixture path"));
        let script = format!(
            "#!{}\nif {} {} 2>/dev/null; then\nprintf '%s' \"$$\" > {}\nwhile [ ! -f {} ]; do {} 0.01; done\nfi\nexec {} \"$@\"\n",
            utility("sh").display(),
            quote(&utility("mkdir")),
            quote(&catalog.cwd.join("gate")),
            quote(&catalog.cwd.join("pid")),
            quote(&catalog.cwd.join("release")),
            quote(&utility("sleep")),
            magi_proto::permit::quoted(&catalog.tooling.program),
        );
        let program = catalog.cwd.join("gated-helper");
        std::fs::write(&program, &script).expect("gate script");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o500)).expect("mode");
        catalog.tooling = Tooling::of(program.to_str().expect("program"));
        catalog.tooling.pin = Some(format!("{:x}", Sha256::digest(script.as_bytes())));
    }).await?;
    install(&fixture, "before cancellation");
    Some(fixture)
}

async fn barrier(reader: &mut Reader, writer: &mut Writer) {
    writer
        .write(&UiCommand::Memory {
            verb: "sessions".into(),
            arg: serde_json::Value::Null,
        })
        .await
        .expect("read barrier");
    event(
        reader,
        |event| matches!(event, HarnessEvent::MemoryAnswered { verb, .. } if verb == "sessions"),
    )
    .await;
}

async fn admitted(reader: &mut Reader, arguments: &str, body: &str) {
    let found = event(reader, |event| {
        matches!(
            event,
            HarnessEvent::WorkflowMessage { .. } | HarnessEvent::Refused { .. }
        )
    })
    .await;
    let HarnessEvent::WorkflowMessage { workflow, .. } = found else {
        panic!("unexpected response: {found:?}");
    };
    assert_eq!(workflow.arguments, arguments);
    assert_eq!(workflow.body, format!("{body}\n"));
    assert_eq!(
        workflow.digest,
        format!("{:x}", Sha256::digest(source(body)))
    );
    idle(reader).await;
}

async fn idle(reader: &mut Reader) {
    event(reader, |event| {
        matches!(
            event,
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn detaching_cancels_active_and_queued_owner_work_without_cancelling_another_controller() {
    let Some(fixture) = gated("owners").await else {
        return;
    };
    let (mut first, mut owner, _) = fixture.attach().await;
    let (mut second, mut other, _) = fixture.attach().await;
    prompt(&mut owner, "/skill review abandoned-active").await;
    let pid = started(&fixture.dir.join("pid")).await;
    prompt(&mut owner, "/skill review abandoned-queued").await;
    barrier(&mut first, &mut owner).await;
    prompt(&mut other, "/skill review retained").await;
    barrier(&mut second, &mut other).await;
    owner.write(&UiCommand::Detach).await.expect("detach owner");
    reaped(pid).await;
    admitted(&mut second, "retained", "before cancellation").await;
    let (_, _, snapshot) = fixture.attach().await;
    assert_eq!(
        snapshot
            .iter()
            .filter(|entry| matches!(entry, Entry::Workflow { .. }))
            .count(),
        1
    );
    assert!(snapshot.iter().all(|entry| !matches!(entry, Entry::Workflow { workflow, .. } if workflow.arguments.starts_with("abandoned"))));
    assert_eq!(fixture.replay().await, snapshot);
}

async fn fresh_after_change(name: &str, change: UiCommand, mode: bool) {
    let Some(fixture) = gated(name).await else {
        return;
    };
    let (mut reader, mut writer, _) = fixture.attach().await;
    prompt(&mut writer, "/skill review old-generation").await;
    let pid = started(&fixture.dir.join("pid")).await;
    writer.write(&change).await.expect("change");
    reaped(pid).await;
    let (mut refused, mut changed) = (false, !mode);
    while !refused || !changed {
        match event(&mut reader, |event| {
            matches!(
                event,
                HarnessEvent::Refused { .. } | HarnessEvent::ModeChanged { .. }
            )
        })
        .await
        {
            HarnessEvent::Refused { message, .. } => {
                assert!(message.contains("preparation"), "{message}");
                refused = true;
            }
            HarnessEvent::ModeChanged { judging, .. } => {
                assert_eq!(judging.mode, magi_proto::judging::Mode::Locked);
                changed = true;
            }
            _ => unreachable!(),
        }
    }
    install(&fixture, "new generation source");
    prompt(&mut writer, "/skill review fresh").await;
    admitted(&mut reader, "fresh", "new generation source").await;
    let (_, _, snapshot) = fixture.attach().await;
    assert_eq!(
        snapshot
            .iter()
            .filter(|entry| matches!(entry, Entry::Workflow { .. }))
            .count(),
        1
    );
    assert_eq!(fixture.replay().await, snapshot);
}

#[tokio::test]
async fn interrupt_allows_fresh_workflow_acquisition_without_reusing_cancelled_state() {
    fresh_after_change("fresh", UiCommand::Interrupt, false).await;
}

#[tokio::test]
async fn successful_policy_change_cancels_old_preparation_and_allows_fresh_selection() {
    fresh_after_change(
        "policy",
        UiCommand::SetMode {
            mode: "locked".into(),
        },
        true,
    )
    .await;
}

#[tokio::test]
async fn successful_resume_restores_frozen_workflow_and_rebinds_new_preparation() {
    let Some(fixture) = gated("resume-ok").await else {
        return;
    };
    let saved = Entry::Workflow {
        id: MessageId::new("saved"),
        text: "/skill review saved".into(),
        aside: "saved aside".into(),
        images: Vec::new(),
        workflow: Box::new(
            magi_host::skills::Loaded {
                source: magi_host::skills::Source {
                    class: magi_host::skills::SourceClass::Project,
                    root: fixture.dir.join(".magi/skills"),
                },
                document: magi_host::skills::parse(
                    "review",
                    source("saved frozen body").as_bytes(),
                )
                .expect("saved source"),
            }
            .snapshot("saved".into())
            .expect("saved snapshot"),
        ),
    };
    let family = Family::dial(fixture.memory.socket())
        .await
        .expect("seed connection");
    let mut target = Scribe::over(
        family,
        Some(fixture.memory.socket().to_owned()),
        &SessionId::new("target"),
    );
    target
        .observe(Cursor(1), &saved, &Default::default())
        .await
        .expect("seed durable snapshot");
    let (mut reader, mut writer, _) = fixture.attach().await;
    prompt(&mut writer, "/skill review old-session").await;
    let pid = started(&fixture.dir.join("pid")).await;
    writer
        .write(&UiCommand::Resume {
            id: "target".into(),
        })
        .await
        .expect("resume");
    reaped(pid).await;
    let (mut resumed, mut cancelled) = (false, false);
    while !resumed || !cancelled {
        match event(&mut reader, |event| {
            matches!(
                event,
                HarnessEvent::SessionSnapshot { .. }
                    | HarnessEvent::Refused { .. }
                    | HarnessEvent::WorkflowMessage { .. }
            )
        })
        .await
        {
            HarnessEvent::SessionSnapshot {
                session, entries, ..
            } => {
                assert!(!resumed);
                assert_eq!(session.as_str(), "target");
                assert_eq!(entries, vec![saved.clone()]);
                resumed = true;
            }
            HarnessEvent::Refused { message, .. } => {
                assert!(!cancelled);
                assert!(message.contains("preparation"), "{message}");
                cancelled = true;
            }
            other => panic!("unexpected resume event: {other:?}"),
        }
    }
    install(&fixture, "after successful resume");
    prompt(&mut writer, "/skill review rebound").await;
    admitted(&mut reader, "rebound", "after successful resume").await;
    let (_, _, snapshot) = fixture.attach().await;
    assert_eq!(snapshot.first(), Some(&saved));
    assert_eq!(
        snapshot
            .iter()
            .filter(|entry| matches!(entry, Entry::Workflow { .. }))
            .count(),
        2
    );
    assert_eq!(fixture.replay_of("target").await, snapshot);
    assert!(fixture.replay().await.is_empty());
}

#[tokio::test]
async fn rewind_restores_first_snapshot_after_changed_source_is_deleted() {
    let Some(fixture) = Fixture::start("rewind", false).await else {
        return;
    };
    let (mut reader, mut writer, _) = fixture.attach().await;
    install(&fixture, "first frozen body");
    prompt(&mut writer, "/skill review first").await;
    admitted(&mut reader, "first", "first frozen body").await;
    let (_, _, first) = fixture.attach().await;
    install(&fixture, "second replaced body");
    prompt(&mut writer, "/skill review second").await;
    admitted(&mut reader, "second", "second replaced body").await;
    std::fs::remove_dir_all(fixture.dir.join(".magi/skills")).expect("delete source");
    writer
        .write(&UiCommand::Branch { keeps: None })
        .await
        .expect("rewind");
    let branched = event(&mut reader, |event| {
        matches!(event, HarnessEvent::Branched { .. })
    })
    .await;
    assert!(matches!(branched, HarnessEvent::Branched { keeps, .. } if keeps == first.len()));
    prompt(&mut writer, "ordinary after rewind").await;
    event(&mut reader, |event| matches!(event, HarnessEvent::UserMessage { text, .. } if text == "ordinary after rewind")).await;
    idle(&mut reader).await;
    let (_, _, snapshot) = fixture.attach().await;
    let context =
        serde_json::to_string(&magi_host::context::of_entries(&snapshot)).expect("context");
    assert!(context.contains("first frozen body"));
    assert!(!context.contains("second replaced body"));
    assert_eq!(fixture.replay().await, snapshot);
    writer
        .write(&UiCommand::Resume {
            id: fixture.id.clone(),
        })
        .await
        .expect("reopen history");
    let resumed = event(&mut reader, |event| {
        matches!(event, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    assert!(
        matches!(resumed, HarnessEvent::SessionSnapshot { entries, .. } if entries == snapshot)
    );
}
