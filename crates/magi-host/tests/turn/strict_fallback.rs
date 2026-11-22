use super::{backend, session, turn};
use magi_ipc::Wire;
use magi_model::RequestFrame;
use magi_proto::{
    HarnessEvent,
    ask::{Ask, Said, Wants},
    caching::Invalidation,
};
use magi_testkit::{
    Mind,
    mind::{retrying_line, stop_line},
};

fn frame(version: u8, disabled: bool) -> String {
    serde_json::to_string(&Said::RequestFrame {
        frame: RequestFrame {
            version,
            dialect: [1; 32],
            policy: [u8::from(disabled); 32],
            omitted_images: 0,
            disable_preferred_strict: disabled,
        },
    })
    .expect("frame")
}

fn options(events: &mut tokio::sync::broadcast::Receiver<HarnessEvent>) -> bool {
    let mut changed = false;
    while let Ok(event) = events.try_recv() {
        if let HarnessEvent::CacheFrame { reasons } = event {
            changed |= reasons.contains(&Invalidation::OptionsChanged);
        }
    }
    changed
}

#[tokio::test]
async fn strict_fallback_reaches_later_asks_and_cache_observations_without_crossing_sessions_or_models()
 {
    let (session, _dir) = session("strict-fallback");
    let (other, _other_dir) = super::session("strict-fallback-other");
    let mind = Mind::turns(
        "strict-fallback",
        &[
            &[
                &frame(1, false),
                &retrying_line(1, 2, 0.0),
                &frame(1, true),
                &stop_line(),
            ],
            &[&frame(1, true), &stop_line()],
        ],
    );
    let backend = backend(&mind);
    let mut events = session.lock().await.subscribe();
    turn(&session, &backend).await;
    assert!(!options(&mut events));
    turn(&session, &backend).await;
    assert!(options(&mut events));
    turn(&session, &backend).await;
    assert!(!options(&mut events));
    let mut different = backend.clone();
    different.model = "fake/two".into();
    turn(&session, &different).await;
    turn(&session, &backend).await;
    turn(&other, &backend).await;
    let asks: Vec<Ask> = mind
        .asks()
        .iter()
        .map(|text| serde_json::from_str(text).expect("ask"))
        .collect();
    assert_eq!(
        asks.iter()
            .map(|ask| ask.wants.disable_preferred_strict)
            .collect::<Vec<_>>(),
        [false, true, true, false, true, false]
    );
    assert!(!backend.wants.disable_preferred_strict);
    for session in [&session, &other] {
        let mut held = session.lock().await;
        for text in [
            serde_json::to_string(held.entries()).expect("entries"),
            serde_json::to_string(&held.take_pending()).expect("pending"),
        ] {
            assert!(!text.contains("disable_preferred_strict"));
            assert!(!text.contains("request_frame"));
        }
    }
    let replacement = Mind::saying("strict-fallback-replacement", &[&stop_line()]);
    let mut replaced = backend.clone();
    replaced.mind = replacement.program().display().to_string();
    turn(&session, &replaced).await;
    let ask: Ask = serde_json::from_str(&replacement.asks()[0]).expect("replacement ask");
    assert!(!ask.wants.disable_preferred_strict);
}

#[tokio::test]
async fn strict_fallback_ignores_unknown_versions_and_legacy_roles_keep_default_requests() {
    let (session, _dir) = session("strict-unknown");
    let mind = Mind::turns(
        "strict-unknown",
        &[&[&frame(2, true), &stop_line()], &[&stop_line()]],
    );
    let backend = backend(&mind);
    for _ in 0..3 {
        turn(&session, &backend).await;
    }
    for text in mind.asks() {
        let ask: Ask = serde_json::from_str(&text).expect("ask");
        assert!(!ask.wants.disable_preferred_strict);
        assert!(!text.contains("disable_preferred_strict"));
    }
}

#[test]
fn strict_fallback_wire_defaults_omit_false_and_survive_json_and_cbor() {
    let wants: Wants = serde_json::from_str("{}").expect("legacy wants");
    assert!(!wants.disable_preferred_strict);
    assert_eq!(serde_json::to_string(&wants).expect("legacy shape"), "{}");
    let mut value: serde_json::Value = serde_json::from_str(&frame(1, false)).expect("frame");
    assert!(value["frame"].get("disable_preferred_strict").is_none());
    for disabled in [false, true] {
        value["frame"]["disable_preferred_strict"] = disabled.into();
        let said: Said = serde_json::from_value(value.clone()).expect("JSON frame");
        let bytes = Wire::Cbor.write(&said).expect("CBOR frame");
        assert_eq!(Wire::read::<Said>(&bytes).expect("CBOR frame"), said);
        let wants = Wants {
            disable_preferred_strict: disabled,
            ..Default::default()
        };
        let bytes = Wire::Cbor.write(&wants).expect("CBOR wants");
        assert_eq!(Wire::read::<Wants>(&bytes).expect("CBOR wants"), wants);
    }
}
