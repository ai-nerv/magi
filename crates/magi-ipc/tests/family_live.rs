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
    let found = looking().await;
    // **`MAGI_REQUIRE_LIVE=1` turns a skip into a failure.** Skipping is right by default — there
    // is no honest verdict when there is nothing real to ask — but it means a green run proves
    // nothing on its own, and these are exactly the tests somebody reaches for to confirm a wire
    // change reached the far side. Setting it says "there is one running, so hold me to it".
    if found.is_none() && std::env::var("MAGI_REQUIRE_LIVE").is_ok_and(|v| v == "1") {
        panic!("MAGI_REQUIRE_LIVE=1 and no balthasar answered; see the skip lines above");
    }
    found
}

/// The first balthasar that answers, if any.
async fn looking() -> Option<Family> {
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

#[tokio::test]
async fn a_real_balthasar_answers_cbor_in_cbor() {
    // The two halves of the family agreeing, over a real socket, in both encodings. Neither
    // repository can prove this alone: magi's own tests can only show that it *writes* CBOR, and
    // balthasar's that it answers what it was handed. What is under test here is that the byte
    // one produces is the byte the other reads.
    let Some(open) = dial().await else {
        return;
    };
    let mut cbor = open.speaking(magi_ipc::Wire::Cbor);
    let verbs = cbor.call("verbs", Vec::new()).await;
    assert!(
        verbs.is_ok(),
        "a balthasar asked in cbor must answer: {verbs:?}"
    );
    let verbs = verbs.expect("verbs");
    assert!(!verbs.is_empty(), "and say something: {verbs:?}");

    // The same connection, still in CBOR, so this is not one lucky frame.
    let again = cbor.call("verbs", Vec::new()).await;
    assert!(again.is_ok(), "and again on the same connection: {again:?}");
}

#[tokio::test]
async fn the_same_question_gets_the_same_answer_in_either_encoding() {
    // One connection, asked twice. Not two connections: a balthasar accepts them one at a time,
    // so a test holding one open while it dials again waits for itself.
    let Some(mut open) = dial().await else {
        return;
    };
    let in_json: Vec<serde_json::Value> = open.call("verbs", Vec::new()).await.expect("json");

    let mut open = open.speaking(magi_ipc::Wire::Cbor);
    let in_cbor: Vec<serde_json::Value> = open.call("verbs", Vec::new()).await.expect("cbor");

    assert_eq!(in_json, in_cbor, "one shape, two encodings");
    assert!(!in_json.is_empty(), "and it said something: {in_json:?}");
}
