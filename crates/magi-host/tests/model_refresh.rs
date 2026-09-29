//! Refresh and model selection through the running session protocol.

use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

async fn attach(
    path: &std::path::Path,
) -> (FrameReader<OwnedReadHalf>, FrameWriter<OwnedWriteHalf>) {
    let stream = magi_ipc::connect(path).await.expect("refresh fixture");
    let (read, write) = stream.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
        })
        .await
        .expect("refresh fixture");
    assert!(matches!(
        next(&mut reader).await,
        HarnessEvent::SessionSnapshot { .. }
    ));
    (reader, writer)
}

async fn next(reader: &mut FrameReader<OwnedReadHalf>) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(5), reader.read())
        .await
        .expect("bounded reply")
        .expect("refresh fixture")
}

#[tokio::test]
async fn refresh_reaches_all_attached_screens_and_new_models_are_selectable() {
    let dir = Scratch::new("magi", "model-refresh-wire");
    let program = dir.join("mind");
    let reply = json!({"ok":true,"refreshed":true,"failed":[],"result":[{
        "id":"ollama/new","provider":"ollama","name":"new","api":"openai-completions",
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
    task.abort();
    let _ = task.await;
}
