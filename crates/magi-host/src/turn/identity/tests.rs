use super::*;
use magi_proto::{SessionId, ask::Card};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

fn backend(capability: Option<bool>, transcript: Option<&str>) -> Backend {
    let mut catalog = crate::catalog::Catalog::empty();
    catalog.cwd = "/project".into();
    catalog.transcript = transcript.map(str::to_owned);
    let mut card: Card = serde_json::from_value(json!({"id":"p/m","provider":"p","name":"m","api":"test","ready":true,"capabilities":{"input":["text"],"output":["text"]}})).expect("card");
    card.capabilities.as_mut().expect("caps").session_identity = capability;
    catalog.cards.push(card);
    catalog.backend("p/m").expect("backend")
}

#[tokio::test]
async fn only_advertised_roles_receive_a_host_owned_opaque_identity() {
    let session = Mutex::new(Session::recorded(SessionId::new("private-session"), vec![]));
    let scribe = Arc::new(Mutex::new(None));
    for capability in [None, Some(false), Some(true)] {
        let backend = backend(capability, None);
        let identity = for_turn(&session, &backend, &scribe).await;
        assert_eq!(identity.is_some(), capability == Some(true));
        if let Some(identity) = identity {
            assert_eq!(identity.len(), 64);
            assert!(identity.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(!identity.contains("private"));
        }
    }
}

#[tokio::test]
async fn tools_model_switches_reconstruction_and_same_journal_resume_keep_identity() {
    let mut backend = backend(Some(true), None);
    let session = Mutex::new(Session::recorded(SessionId::new("s"), vec![]));
    let scribe = Arc::new(Mutex::new(None));
    let original = for_turn(&session, &backend, &scribe).await;
    backend.model = "another/model".into();
    backend.mind = "another-model-role".into();
    backend.wants.thinking = Some(magi_model::ThinkingLevel::High);
    assert_eq!(for_turn(&session, &backend, &scribe).await, original);
    session
        .lock()
        .await
        .resume_prepared(magi_journal::Journal::recorded(SessionId::new("s"), vec![]));
    assert_eq!(for_turn(&session, &backend, &scribe).await, original);
    let restored = Mutex::new(Session::restored(SessionId::new("s"), vec![]).expect("restore"));
    assert_eq!(for_turn(&restored, &backend, &scribe).await, original);
    session
        .lock()
        .await
        .resume_prepared(magi_journal::Journal::recorded(
            SessionId::new("other"),
            vec![],
        ));
    assert_ne!(for_turn(&session, &backend, &scribe).await, original);
}

#[tokio::test]
async fn durable_transcripts_override_run_ids_and_stale_backend_transcript_after_resume() {
    let session = Mutex::new(Session::recorded(SessionId::new("shared-run"), vec![]));
    let backend = backend(Some(true), Some("stale-config"));
    let scribe = Arc::new(Mutex::new(Some(
        crate::scribe::Scribe::unconnected("/memory/socket".into(), &SessionId::new("shared-run"))
            .recording_as("run@one".into()),
    )));
    let first = for_turn(&session, &backend, &scribe).await;
    *scribe.lock().await = Some(
        crate::scribe::Scribe::unconnected("/memory/socket".into(), &SessionId::new("shared-run"))
            .recording_as("run@two".into()),
    );
    let second = for_turn(&session, &backend, &scribe).await;
    assert_ne!(first, second);
    *scribe.lock().await = Some(
        crate::scribe::Scribe::unconnected(
            "/memory/socket".into(),
            &SessionId::new("recreated-runtime"),
        )
        .recording_as("run@one".into()),
    );
    assert_eq!(for_turn(&session, &backend, &scribe).await, first);
}

#[tokio::test]
async fn unrecorded_agents_and_projects_do_not_share_scope_and_disabled_roles_do_not_lock() {
    let session = Mutex::new(Session::recorded(SessionId::new("shared-run"), vec![]));
    let scribe = Arc::new(Mutex::new(None));
    let first = for_turn(&session, &backend(Some(true), Some("run@one")), &scribe).await;
    assert_ne!(
        for_turn(&session, &backend(Some(true), Some("run@two")), &scribe).await,
        first
    );
    let mut other = backend(Some(true), Some("run@one"));
    other.cwd = "/other".into();
    assert_ne!(for_turn(&session, &other, &scribe).await, first);
    let _held = scribe.lock().await;
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            for_turn(&session, &backend(None, None), &scribe)
        )
        .await
        .expect("no lock"),
        None
    );
}
