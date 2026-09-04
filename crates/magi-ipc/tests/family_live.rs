//! The family client against a balthasar that is actually running.
//!
//! Skipped when there is none, because the framing is what these prove and a mock would be magi
//! agreeing with magi. Point them at one with `MAGI_API_SOCKET`, or let them find one under
//! `$XDG_RUNTIME_DIR/balthasar`.

use magi_ipc::family::{Family, Fault, candidates};
use std::time::Duration;

/// How long a socket has to answer before this decides there is nothing behind it.
///
/// **Dialling is not liveness.** A unix socket whose owner has wedged, or is halfway through
/// shutting down, accepts instantly and then never answers — or answers by hanging up. Taking the
/// connection as proof is how these four tests failed a whole suite with `Connection reset by
/// peer` while reporting it as a broken wire.
///
/// It is a shared directory and a workspace run fills it: every test that spawns `magi` starts a
/// balthasar of its own in there, so at any moment several of these sockets belong to a process
/// that is about to be killed. [`magi-host`'s `scribe_live`] learned this first and probes the
/// same way, for the same reason.
const ANSWERS_WITHIN: Duration = Duration::from_secs(3);

/// How many to try before deciding none of them is a live balthasar.
///
/// Walked rather than taking the newest and giving up on it, which is what left these skipping —
/// or failing — whenever the most recent socket happened to belong to a dying test. Bounded so a
/// directory full of dead ones costs seconds rather than minutes.
const AT_MOST: usize = 4;

/// The first balthasar that answers, or `None` and a line saying why each one did not.
///
/// Skipping rather than failing: these prove the framing against something real, and there is no
/// honest verdict to give when there is nothing real to ask.
async fn dial() -> Option<Family> {
    for path in candidates(None).into_iter().take(AT_MOST) {
        let mut family = match Family::dial(&path).await {
            Ok(open) => open,
            Err(e) => {
                eprintln!("skipping {}: {e}", path.display());
                continue;
            }
        };
        // One cheap round trip. What is being asked is not what it answers but whether it answers
        // at all — and `verbs` is the one call every balthasar has had since v1.
        match tokio::time::timeout(ANSWERS_WITHIN, family.call("verbs", Vec::new())).await {
            Ok(Ok(_)) => return Some(family),
            Ok(Err(e)) => eprintln!("skipping {}: refused: {e}", path.display()),
            Err(_) => eprintln!("skipping {}: accepted and did not answer", path.display()),
        }
    }
    None
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
