use super::{backend, session, turn};
use magi_model::RequestFrame;
use magi_proto::{HarnessEvent, caching::Invalidation};
use magi_testkit::{
    Mind,
    mind::{retrying_line, stop_line, text_line},
};

fn frame(dialect: u8, policy: u8, images: u64) -> String {
    serde_json::to_string(&magi_proto::ask::Said::RequestFrame {
        frame: RequestFrame {
            version: 1,
            dialect: [dialect; 32],
            policy: [policy; 32],
            omitted_images: images,
        },
    })
    .expect("frame")
}

fn observed(events: &mut tokio::sync::broadcast::Receiver<HarnessEvent>) -> Vec<Vec<Invalidation>> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let HarnessEvent::ProviderFrame { reasons } = event {
            out.push(reasons);
        }
    }
    out
}

#[tokio::test]
async fn changed_policy_behind_one_model_is_reported_without_storing_its_fingerprints() {
    let (session, _dir) = session("provider-policy");
    let mind = Mind::turns(
        "provider-policy",
        &[
            &[&frame(1, 2, 0), &text_line("first"), &stop_line()],
            &[&frame(1, 2, 0), &text_line("second"), &stop_line()],
            &[&frame(3, 2, 0), &text_line("third"), &stop_line()],
            &[&frame(3, 4, 1), &text_line("fourth"), &stop_line()],
        ],
    );
    let backend = backend(&mind);
    let mut events = session.lock().await.subscribe();
    for expected in [
        vec![],
        vec![],
        vec![Invalidation::DialectChanged],
        vec![
            Invalidation::ProviderPolicyChanged,
            Invalidation::ImagesAdapted,
        ],
    ] {
        turn(&session, &backend).await;
        assert_eq!(observed(&mut events), [expected]);
    }
    let mut held = session.lock().await;
    let stored = serde_json::to_string(held.entries()).expect("entries");
    let pending = serde_json::to_string(&held.take_pending()).expect("pending");
    for text in [stored, pending, mind.heard()] {
        assert!(!text.contains("request_frame"));
        assert!(!text.contains("omitted_images"));
        assert!(!text.contains("dialect"));
    }
}

#[tokio::test]
async fn missing_unknown_and_malformed_metadata_clear_the_comparison_baseline() {
    let (session, _dir) = session("provider-unknown");
    let unknown = frame(9, 9, 0).replace("\"version\":1", "\"version\":2");
    let malformed = frame(9, 9, 0).replace("\"policy\":[9,", "\"policy\":[");
    let mind = Mind::turns(
        "provider-unknown",
        &[
            &[&frame(1, 1, 0), &stop_line()],
            &[&stop_line()],
            &[&frame(2, 2, 0), &stop_line()],
            &[&unknown, &stop_line()],
            &[&frame(3, 3, 0), &stop_line()],
            &[&malformed, &stop_line()],
            &[&frame(4, 4, 0), &stop_line()],
        ],
    );
    let backend = backend(&mind);
    let mut events = session.lock().await.subscribe();
    for nth in 0..7 {
        turn(&session, &backend).await;
        let expected = if nth % 2 == 0 {
            vec![]
        } else {
            vec![Invalidation::ObservationUnavailable]
        };
        assert_eq!(observed(&mut events), [expected], "request {nth}");
    }
}

#[tokio::test]
async fn retry_metadata_is_ordered_but_never_becomes_response_content() {
    let (session, _dir) = session("provider-retry");
    let mind = Mind::saying(
        "provider-retry",
        &[
            &frame(1, 1, 0),
            &text_line("discarded"),
            &retrying_line(1, 2, 0.0),
            &frame(1, 2, 2),
            &text_line("kept"),
            &stop_line(),
        ],
    );
    let mut events = session.lock().await.subscribe();
    turn(&session, &backend(&mind)).await;
    assert_eq!(
        observed(&mut events),
        [
            vec![],
            vec![
                Invalidation::ProviderPolicyChanged,
                Invalidation::ImagesAdapted
            ]
        ]
    );
    let held = session.lock().await;
    assert_eq!(held.entries().len(), 1);
    assert!(
        matches!(&held.entries()[0], magi_proto::Entry::Assistant { text, .. } if text=="kept")
    );
}

#[tokio::test]
async fn a_retry_without_metadata_does_not_reuse_the_failed_attempts_observation() {
    let (session, _dir) = session("provider-retry-missing");
    let mind = Mind::saying(
        "provider-retry-missing",
        &[
            &frame(1, 1, 0),
            &retrying_line(1, 2, 0.0),
            &text_line("kept"),
            &stop_line(),
        ],
    );
    let mut events = session.lock().await.subscribe();
    turn(&session, &backend(&mind)).await;
    assert_eq!(
        observed(&mut events),
        [vec![], vec![Invalidation::ObservationUnavailable]]
    );
}

#[tokio::test]
async fn pre_send_metadata_does_not_release_deferred_helpers() {
    let (session, _dir) = session("provider-before-http");
    let mind = Mind::stalling("provider-before-http", &[&frame(1, 1, 0)]);
    let backend = backend(&mind);
    session.lock().await.defer(vec![magi_proto::laying::Job {
        id: "still deferred".into(),
        ..Default::default()
    }]);
    let mut events = session.lock().await.subscribe();
    let inspect = async {
        loop {
            if matches!(
                events.recv().await.expect("frame event"),
                HarnessEvent::ProviderFrame { .. }
            ) {
                let mut held = session.lock().await;
                assert_eq!(held.take_deferred().len(), 1);
                held.cancel().request();
                break;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(turn(&session, &backend), inspect);
    })
    .await
    .expect("bounded cancellation");
}
