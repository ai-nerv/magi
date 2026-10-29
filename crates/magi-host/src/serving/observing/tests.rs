#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::*;
use crate::events::{Event, Receiver, decode, enable_local};

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut events = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        events.push(decode(&bytes).expect("strict event"));
    }
    events
}

#[test]
fn refusal_and_unpolled_handler_drop_have_distinct_lifecycles() {
    if std::env::var_os("MAGI_EVENT_CLIENT_PROBE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "serving::observing::tests::refusal_and_unpolled_handler_drop_have_distinct_lifecycles"])
            .env("MAGI_EVENT_CLIENT_PROBE", "1")
            .output().expect("probe");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("test result: ok. 1 passed; 0 failed; 0 ignored;")
        );
        return;
    }
    let receiver = enable_local().expect("activation");
    let client = Client::connected();
    let future = async move {
        let _client = client;
        std::future::pending::<()>().await;
    };
    drop(future);
    refused();
    let events = drain(&receiver);
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].correlation, events[1].correlation);
    assert_ne!(
        events[0].correlation.request_id,
        events[2].correlation.request_id
    );
    for (event, expected) in events.into_iter().zip([
        ClientOutcome::Connected,
        ClientOutcome::Disconnected,
        ClientOutcome::Refused,
    ]) {
        assert!(event.correlation.request_id.is_some());
        assert_eq!(event.fact, Fact::Client { outcome: expected });
    }
}
