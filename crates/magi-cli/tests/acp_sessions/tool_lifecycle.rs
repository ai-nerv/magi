use super::{turns::*, *};

#[path = "../support/tool_process.rs"]
mod tool_process;

async fn working(fixture: &mut Fixture, name: &str) -> (Value, magi_testkit::Mind) {
    let barrier = fixture.root.join("a/tool-barrier");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &barrier,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("tool barrier FIFO");
    let command = format!(
        "printf started > '{}'; cat '{}'; printf completed > '{}'",
        fixture.root.join("a/tool-started").display(),
        barrier.display(),
        fixture.root.join("a/tool-completed").display()
    );
    let mind = tooling(name, "shell", json!({"command":command}));
    synthetic(fixture, &mind);
    let session = session(fixture).await;
    prompt(fixture, 3, &session, "start a tool and hold it").await;
    loop {
        let frame = fixture.read().await;
        if frame["method"] == "session/request_permission" {
            assert_eq!(frame["params"]["sessionId"], session);
            let option = frame["params"]["options"]
                .as_array()
                .expect("permission options")
                .iter()
                .find(|option| option["kind"] == "allow_once")
                .expect("allow once");
            fixture
                .send(json!({"jsonrpc":"2.0", "id":frame["id"], "result":{
                    "outcome":{"outcome":"selected", "optionId":option["optionId"]}
                }}))
                .await;
            break;
        }
        assert!(frame.get("id").is_none(), "early completion: {frame}");
    }
    (session, mind)
}

async fn finished(fixture: &mut Fixture, id: u64, reason: &str) {
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(
                frame["id"], id,
                "no unrelated request or duplicate completion"
            );
            assert_eq!(frame["result"]["stopReason"], reason, "{frame}");
            return;
        }
    }
}

#[tokio::test]
async fn native_acp_cancel_during_running_tool_reaps_it_before_recovery() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let (session, mind) = working(&mut fixture, "acp-running-tool-cancel").await;
    let readers = tool_process::started(&fixture.root)
        .await
        .expect("running tool");
    assert_eq!(mind.asked(), 1, "no followup while the tool is held");
    assert!(!fixture.root.join("a/tool-completed").exists());
    fixture
        .send(json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":session}}))
        .await;
    finished(&mut fixture, 3, "cancelled").await;
    tool_process::reaped(&readers).await.expect("reaped tool");
    assert_eq!(
        mind.asked(),
        1,
        "cancellation cannot start a provider followup"
    );
    prompt(&mut fixture, 4, &session, "recover after the held tool").await;
    finished(&mut fixture, 4, "end_turn").await;
    assert_eq!(mind.asked(), 2);
    assert!(!mind.overlapped());
    assert!(mind.asks()[1].contains("recover after the held tool"));
    let root = fixture.close().await;
    assert!(!root.join("a/tool-completed").exists());
}

#[tokio::test]
async fn native_acp_disconnect_during_running_tool_reaps_it_without_followup() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let (_, mind) = working(&mut fixture, "acp-running-tool-disconnect").await;
    let readers = tool_process::started(&fixture.root)
        .await
        .expect("running tool");
    assert_eq!(mind.asked(), 1);
    let root = fixture.close().await;
    tool_process::reaped(&readers).await.expect("reaped tool");
    assert_eq!(mind.asked(), 1);
    assert!(!root.join("a/tool-completed").exists());
}
