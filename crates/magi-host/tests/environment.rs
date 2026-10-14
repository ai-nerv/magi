#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{catalog::Catalog, session::Session};
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, Environment, HarnessEvent, SessionId, UiCommand};
use std::os::unix::fs::PermissionsExt;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

async fn event(reader: &mut Reader, wanted: impl Fn(&HarnessEvent) -> bool) -> HarnessEvent {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let event = reader.read().await.expect("host event");
            assert!(
                !serde_json::to_string(&event)
                    .expect("wire event")
                    .contains("private-sentinel")
            );
            if wanted(&event) {
                return event;
            }
        }
    })
    .await
    .expect("host barrier")
}

async fn command(reader: &mut Reader, writer: &mut Writer, source: bool, input: &str) -> String {
    writer
        .write(&UiCommand::Environment(Environment {
            source,
            input: input.into(),
        }))
        .await
        .expect("environment command");
    match event(reader, |e| matches!(e, HarnessEvent::Noticed { .. })).await {
        HarnessEvent::Noticed { text, .. } => text,
        _ => unreachable!(),
    }
}

async fn verify_turn(reader: &mut Reader, writer: &mut Writer) {
    writer
        .write(&UiCommand::SubmitPrompt {
            text: "verify environment".into(),
            aside: String::new(),
            images: Vec::new(),
        })
        .await
        .expect("prompt");
    event(reader, |e| matches!(e, HarnessEvent::AssistantDelta { text, .. } if text == "environment verified")).await;
    event(reader, |e| matches!(e, HarnessEvent::AssistantEnded { .. })).await;
    event(reader, |e| {
        matches!(
            e,
            HarnessEvent::StatusChanged {
                status: magi_proto::AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn environment_changes_reach_real_provider_children_and_refresh_without_recording_secrets() {
    let dir = Scratch::new("magi-env", "ipc");
    let provider = dir.join("mind");
    let card = serde_json::json!({
        "id":"fixture/one", "provider":"fixture", "name":"one", "api":"openai-completions",
        "context_window":200000, "max_output":1000, "ready":false, "needs":"OPENROUTER_API_KEY",
        "vision":false, "reasons":false
    });
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n\
         models) ready=false; [ -n \"$OPENROUTER_API_KEY\" ] && ready=true; printf '%s\\n' '{}' | sed \"s/\\\"ready\\\":false/\\\"ready\\\":$ready/\";;\n\
         ask) /bin/cat > /dev/null; [ \"$OPENROUTER_API_KEY\" = \"$(cat '{}')\" ] || exit 1; \
         [ \"$MAGI_ENV_PATH\" = \"new:old:tail\" ] || exit 2; \
         printf '%s\\n' '{{\"event\":\"text\",\"text\":\"environment verified\"}}' '{{\"event\":\"stop\",\"reason\":\"end_turn\"}}';;\n\
         *) exit 3;;\nesac\n",
        serde_json::json!({"ok":true,"refreshed":true,"failed":[],"result":[card.clone()]}),
        dir.join("expected").display()
    );
    std::fs::write(&provider, script).expect("provider fixture");
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700))
        .expect("fixture executable");
    let mut catalog = Catalog::empty();
    catalog.mind = provider.display().to_string();
    catalog.tooling.program = "/magi-no-tool-provider".into();
    catalog.cwd = dir.to_path_buf();
    catalog.cards = vec![serde_json::from_value(card).expect("card")];
    catalog.chosen = Some("fixture/one".into());
    catalog
        .environ
        .insert("MAGI_ENV_PATH".into(), "old:tail".into());
    catalog
        .environ
        .insert("OPENROUTER_API_KEY".into(), String::new());
    let path = dir.join("s.sock");
    let serving = tokio::spawn(magi_host::serve_on(
        magi_ipc::bind(&path).await.expect("bind"),
        Session::recorded(SessionId::new("env"), Vec::new()),
        None,
        catalog,
        Some(dir.join("absent-memory.sock")),
    ));
    let stream = magi_ipc::connect(&path).await.expect("connect");
    let (read, write) = stream.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
            key: magi_ipc::read_key(&path),
        })
        .await
        .expect("attach");
    event(&mut reader, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    let notice = command(
        &mut reader,
        &mut writer,
        false,
        "OPENROUTER_API_KEY=private-sentinel MAGI_ENV_PATH=new:$MAGI_ENV_PATH",
    )
    .await;
    assert!(notice.contains("Session variables set"));
    event(&mut reader, |e| matches!(e, HarnessEvent::ModelsRefreshed { choices, warning: None } if choices[0].requirement.is_empty())).await;
    std::fs::write(dir.join("expected"), "private-sentinel").expect("expected environment");
    verify_turn(&mut reader, &mut writer).await;
    std::fs::write(
        dir.join("exports file.sh"),
        "echo ignored > executed\nf() { export OPENROUTER_API_KEY=wrong; }\n\
         export OPENROUTER_API_KEY=updated:$OPENROUTER_API_KEY\n\
         export UNSAFE=$(touch executed)\n",
    )
    .expect("export fixture");
    let notice = command(&mut reader, &mut writer, true, "\"exports file.sh\"").await;
    assert!(notice.contains("Skipped 1"), "{notice}");
    assert!(!dir.join("executed").exists());
    std::fs::write(dir.join("expected"), "updated:private-sentinel").expect("expected environment");
    verify_turn(&mut reader, &mut writer).await;
    let notice = command(
        &mut reader,
        &mut writer,
        false,
        "OPENROUTER_API_KEY=wrong BAD=$(touch executed)",
    )
    .await;
    assert!(notice.contains("no variables changed"));
    verify_turn(&mut reader, &mut writer).await;
    writer
        .write(&UiCommand::Environment(Environment {
            source: true,
            input: "does-not-exist".into(),
        }))
        .await
        .expect("missing file");
    event(
        &mut reader,
        |e| matches!(e, HarnessEvent::Noticed { text, .. } if text.contains("could not open")),
    )
    .await;
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
            key: magi_ipc::read_key(&path),
        })
        .await
        .expect("repeat attach");
    let stream = magi_ipc::connect(&path).await.expect("snapshot connection");
    let (read, write) = stream.into_split();
    let mut snapshots = FrameReader::new(read);
    FrameWriter::new(write)
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
            key: magi_ipc::read_key(&path),
        })
        .await
        .expect("snapshot");
    event(&mut snapshots, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    assert!(!dir.join("executed").exists());
    serving.abort();
}
