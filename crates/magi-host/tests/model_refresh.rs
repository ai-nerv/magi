//! Refresh and model selection through the running session protocol.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

#[path = "model_refresh/thinking.rs"]
mod thinking;

#[path = "model_refresh/input_limits.rs"]
mod input_limits;

async fn attach(
    path: &std::path::Path,
) -> (FrameReader<OwnedReadHalf>, FrameWriter<OwnedWriteHalf>) {
    let (reader, writer, snapshot) = attached(path).await;
    assert!(matches!(snapshot, HarnessEvent::SessionSnapshot { .. }));
    (reader, writer)
}

async fn attached(
    path: &std::path::Path,
) -> (
    FrameReader<OwnedReadHalf>,
    FrameWriter<OwnedWriteHalf>,
    HarnessEvent,
) {
    let stream = magi_ipc::connect(path).await.expect("refresh fixture");
    let (read, write) = stream.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
            key: magi_ipc::read_key(path),
        })
        .await
        .expect("refresh fixture");
    let snapshot = next(&mut reader).await;
    (reader, writer, snapshot)
}

async fn next(reader: &mut FrameReader<OwnedReadHalf>) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(5), reader.read())
        .await
        .expect("bounded reply")
        .expect("refresh fixture")
}

#[tokio::test]
async fn refresh_reaches_all_attached_screens_and_new_models_are_selectable() {
    if thinking::isolated("refresh_reaches_all_attached_screens_and_new_models_are_selectable") {
        return;
    }
    let dir = Scratch::new("magi", "model-refresh-wire");
    let program = dir.join("mind");
    let reply = json!({"ok":true,"refreshed":true,"failed":[],"result":[{
        "id":"ollama/new","provider":"ollama","name":"new","display_name":"Display label, not a route","api":"openai-completions",
        "ready":true,"reasons":false,"context_window":32000,"max_output":8000
    }]});
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\n[ \"$*\" = \"models --json --refresh\" ] || exit 1\nprintf '%s\\n' '{}'\n",
            reply
        ),
    )
    .expect("refresh fixture");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
        .expect("refresh fixture");
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.mind = program.to_string_lossy().into_owned();
    catalog.memory = "/fixture-no-real-memory".into();
    catalog.tooling.program = "/fixture-no-real-tools".into();
    catalog.cwd = dir.to_path_buf();
    let path = dir.join("s.sock");
    let listener = magi_ipc::bind(&path).await.expect("refresh fixture");
    let task = tokio::spawn(magi_host::serve_catalog(
        listener,
        magi_host::open_session(1, ""),
        None,
        catalog,
    ));
    let (mut first, mut writer) = attach(&path).await;
    let (mut second, _other_writer) = attach(&path).await;
    writer
        .write(&UiCommand::RefreshModels)
        .await
        .expect("refresh fixture");
    for reader in [&mut first, &mut second] {
        let HarnessEvent::ModelsRefreshed { choices, warning } = next(reader).await else {
            panic!("refresh event")
        };
        assert_eq!(choices[0].name, "ollama/new");
        assert_eq!(
            choices[0].display_name.as_deref(),
            Some("Display label, not a route")
        );
        assert!(warning.is_none());
    }
    writer
        .write(&UiCommand::SetModel {
            name: "ollama/new".into(),
        })
        .await
        .expect("refresh fixture");
    let event = next(&mut first).await;
    assert!(
        matches!(event, HarnessEvent::ModelChanged { .. }),
        "{event:?}"
    );
    let (_, _, snapshot) = attached(&path).await;
    let HarnessEvent::SessionSnapshot { model, choices, .. } = snapshot else {
        panic!("snapshot");
    };
    assert_eq!(model.expect("model").name, "ollama/new");
    assert_eq!(
        choices[0].display_name.as_deref(),
        Some("Display label, not a route")
    );
    task.abort();
    let _ = task.await;
}
