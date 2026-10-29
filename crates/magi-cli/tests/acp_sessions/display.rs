use super::*;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{HarnessEvent, UiCommand};
use magi_testkit::Mind;

#[tokio::test]
async fn native_acp_rewind_and_user_images_remain_visible_without_unsolicited_replies() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::answering("acp-display", "native display answer");
    turns::synthetic(&fixture, &mind);
    let session = turns::session(&mut fixture).await;
    let socket = std::fs::read_dir(fixture.root.join("r/magi"))
        .expect("runtime")
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with("acp-"))
        .expect("owned host")
        .path()
        .join("host");
    let (read, write) = magi_ipc::connect(&socket)
        .await
        .expect("controller socket")
        .into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::AttachLatest {
            draws: false,
            key: Some(magi_ipc::read_key(&socket).expect("controller key")),
        })
        .await
        .expect("attach");
    let HarnessEvent::SessionSnapshot {
        session: native,
        cursor,
        ..
    } = reader.read::<HarnessEvent>().await.expect("snapshot")
    else {
        panic!("expected snapshot");
    };
    let (probe_read, probe_write) = magi_ipc::connect(&socket)
        .await
        .expect("history socket")
        .into_split();
    FrameWriter::new(probe_write)
        .write(&UiCommand::ReadHistory {
            session: native,
            through: cursor,
            after: cursor,
            key: Some(magi_ipc::read_key(&socket).expect("controller key")),
        })
        .await
        .expect("history identity");
    let page = FrameReader::new(probe_read)
        .read::<Result<magi_proto::HistoryPage, String>>()
        .await
        .expect("history response")
        .expect("history identity accepted");
    let transcript = page.transcript.expect("durable transcript key");
    let encoded: String = transcript
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(session, format!("magi-v1-{encoded}"));
    assert!(page.entries.is_empty() && page.next.is_none());
    writer
        .write(&UiCommand::Branch { keeps: Some(0) })
        .await
        .expect("native rewind");
    let rewind = fixture.read().await;
    assert_eq!(rewind["params"]["sessionId"], session);
    assert_eq!(
        rewind["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert!(
        rewind["params"]["update"]["content"]["text"]
            .as_str()
            .expect("rewind text")
            .contains("Context rewound")
    );
    assert!(rewind.get("id").is_none());
    let image = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    writer
        .write(&UiCommand::SubmitPrompt {
            text: "native image prompt".into(),
            aside: "private-display-context".into(),
            images: vec![magi_model::Image {
                data: image.into(),
                media_type: "image/png".into(),
            }],
        })
        .await
        .expect("native prompt");
    let mut saw_image = false;
    let mut saw_prompt = false;
    loop {
        let frame = fixture.read().await;
        assert!(
            frame.get("id").is_none(),
            "no invented foreground response: {frame}"
        );
        assert_eq!(frame["params"]["sessionId"], session);
        assert!(!frame.to_string().contains("private-display-context"));
        let update = &frame["params"]["update"];
        let _: agent_client_protocol_schema::v1::SessionNotification =
            serde_json::from_value(frame["params"].clone()).expect("official frame");
        if update["content"]["type"] == "image" {
            assert_eq!(update["sessionUpdate"], "user_message_chunk");
            assert_eq!(update["content"]["data"], image);
            saw_image = true;
        }
        saw_prompt |= update["content"]["text"] == "native image prompt";
        if update["content"]["text"] == "native display answer" {
            break;
        }
    }
    assert!(saw_image && saw_prompt);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(
            reader.read::<HarnessEvent>().await.expect("native event"),
            HarnessEvent::TurnFinished { .. }
        ) {}
    })
    .await
    .expect("native settlement");
    writer
        .write(&UiCommand::RefreshModels)
        .await
        .expect("ordered native refresh");
    let barrier = fixture.read().await;
    assert!(barrier.get("id").is_none(), "{barrier}");
    assert_eq!(barrier["params"]["sessionId"], session);
    assert_eq!(
        barrier["params"]["update"]["sessionUpdate"],
        "config_option_update"
    );
    assert_eq!(
        barrier["params"]["update"]["configOptions"][0]["currentValue"],
        "fake/one"
    );
    let _: agent_client_protocol_schema::v1::SessionNotification =
        serde_json::from_value(barrier["params"].clone()).expect("ordered ACP barrier");
    drop((reader, writer));
    turns::prompt(&mut fixture, 3, &session, "following foreground prompt").await;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 3);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            break;
        }
    }
    assert_eq!(mind.asked(), 2);
    assert!(mind.asks()[0].contains("private-display-context"));
    fixture.close().await;
}
