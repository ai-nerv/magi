//! The family client against a balthasar that is actually running.
//!
//! Skipped when there is no socket, because the framing is what these prove and a mock would be
//! magi agreeing with magi. Point them at one with `MAGI_API_SOCKET`, or let them find the
//! newest under `$XDG_RUNTIME_DIR/balthasar`.

use magi_ipc::family::{Family, Fault, candidates};

/// Connect, or say why the test is being skipped rather than failing for the wrong reason.
async fn dial() -> Option<Family> {
    let path = candidates(None).into_iter().next()?;
    match Family::dial(&path).await {
        Ok(open) => Some(open),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

#[tokio::test]
async fn a_running_balthasar_lists_its_verbs() {
    let Some(mut family) = dial().await else {
        return;
    };
    let values = family.call("verbs", Vec::new()).await.expect("verbs");
    let listed = format!("{values:?}");
    for verb in ["observe", "amend", "replay", "resume", "sessions"] {
        assert!(listed.contains(verb), "{verb} missing from {listed}");
    }
}

#[tokio::test]
async fn one_connection_serves_many_calls() {
    let Some(mut family) = dial().await else {
        return;
    };
    for _ in 0..3 {
        family.call("verbs", Vec::new()).await.expect("verbs again");
    }
}

#[tokio::test]
async fn a_verb_that_does_not_exist_is_refused_rather_than_disconnecting() {
    let Some(mut family) = dial().await else {
        return;
    };
    let answer = family.call("no_such_verb_at_all", Vec::new()).await;
    assert!(
        matches!(answer, Err(Fault::Refused(_))),
        "expected a refusal, got {answer:?}"
    );
    family
        .call("verbs", Vec::new())
        .await
        .expect("the connection survives a refusal");
}

#[tokio::test]
async fn a_turn_can_be_observed_and_replayed() {
    let Some(mut family) = dial().await else {
        return;
    };
    let session = format!("magi-family-{}", std::process::id());
    let turn = serde_json::json!({
        "cursor": 1,
        "role": "user",
        "kind": "prose",
        "text": "does the wire hold",
        "raw": { "record": "entry", "cursor": 1 },
    });

    family
        .call(
            "observe",
            vec![serde_json::Value::String(session.clone()), turn],
        )
        .await
        .expect("observe");

    let back = family
        .call("replay", vec![serde_json::Value::String(session)])
        .await
        .expect("replay");
    assert!(
        format!("{back:?}").contains("does the wire hold"),
        "what went in did not come back: {back:?}"
    );
}
