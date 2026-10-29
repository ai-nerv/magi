use super::*;
use magi_host::events::{self, Correlation, Event, Fact, Receiver, TurnOutcome};

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut result = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        assert!(bytes.len() <= events::MAX_EVENT_BYTES);
        let text = std::str::from_utf8(&bytes).expect("JSON");
        for secret in ["private", "SKILL", "review", "HOME", "PATH"] {
            assert!(!text.contains(secret));
        }
        result.push(events::decode(&bytes).expect("strict event"));
    }
    result
}

#[tokio::test]
async fn workflow_preparation_keeps_one_identity_through_native_history() {
    if std::env::var_os("MAGI_WORKFLOW_EVENT_PROBE").is_none() {
        for name in [
            "CASPER_TEST_BINARY",
            "BALTHASAR_TEST_BINARY",
            "CASPER_TEST_TOOLS",
        ] {
            if selected(name).is_none() {
                return;
            }
        }
        let mut child = std::process::Command::new(std::env::current_exe().expect("executable"))
            .args([
                "--exact",
                "observing::workflow_preparation_keeps_one_identity_through_native_history",
            ])
            .env("MAGI_WORKFLOW_EVENT_PROBE", "1")
            .spawn()
            .expect("isolated probe");
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = child.try_wait().expect("status") {
                assert!(status.success());
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("workflow probe timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let Some(fixture) = Fixture::start("ev", false).await else {
        return;
    };
    let path = fixture.dir.join(".magi/skills/review/SKILL.md");
    put(
        &path,
        "---\nname: review\ndescription: private-description\n---\nprivate-frozen-body\n",
    );
    let receiver = events::enable_local().expect("activation");
    let (mut reader, mut writer, entries) = fixture.attach().await;
    assert!(entries.is_empty());
    let connected = drain(&receiver);
    assert_eq!(connected.len(), 1);
    assert_eq!(
        connected[0].fact,
        Fact::Client {
            outcome: events::ClientOutcome::Connected
        }
    );
    prompt(&mut writer, "/skill review private-arguments").await;
    event(&mut reader, |event| {
        matches!(event, HarnessEvent::TurnFinished { .. })
    })
    .await;
    let observed = drain(&receiver);
    let expected = [
        TurnOutcome::Preparing,
        TurnOutcome::Admitted,
        TurnOutcome::Started,
        TurnOutcome::Failed,
    ];
    assert_eq!(observed.len(), expected.len());
    let id = observed[0].correlation.turn_id.expect("logical identity");
    for (event, outcome) in observed.into_iter().zip(expected) {
        assert_eq!(
            event.correlation,
            Correlation {
                turn_id: Some(id),
                ..Correlation::default()
            }
        );
        let Fact::LogicalTurn {
            outcome: actual,
            duration_ms,
        } = event.fact
        else {
            panic!("logical turn")
        };
        assert_eq!(actual, outcome);
        assert_eq!(duration_ms.is_some(), outcome == TurnOutcome::Failed);
    }
    let before = fixture.replay().await;
    assert!(
        before
            .iter()
            .any(|entry| matches!(entry, Entry::Workflow { .. }))
    );
    assert!(
        before
            .iter()
            .any(|entry| matches!(entry, Entry::Assistant { error: Some(_), .. }))
    );
    let encoded = serde_json::to_string(&before).expect("history");
    assert!(encoded.contains("private-frozen-body") && encoded.contains("private-arguments"));
    put(&path, "changed-private-source");
    assert_eq!(fixture.replay().await, before);
    assert!(drain(&receiver).is_empty());
    assert_eq!(events::loss(), events::Loss::default());
    drop(reader);
    drop(writer);
}
