//! What the turn tells balthasar about how the memory it was handed got used. Best effort on the
//! turn's own clock: a session with no balthasar behaves as it did before one.

/// How long an outcome report may hold up a turn. Instrumentation, in front of the person.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(250);

/// Report one finished tool against the injection that preceded it. The action is one string,
/// because that is what balthasar hashes; the arguments do not leave this process. `recall` and
/// `remember` are skipped, or every injection would look used.
pub(super) async fn acted_on(
    scribe: &crate::scribe::Held,
    injection: &str,
    call: &magi_core::PendingCall,
    failed: bool,
) {
    if matches!(call.name.as_str(), "recall" | "remember" | "forget" | "why") {
        return;
    }
    let action = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .ok()
        .and_then(|args| {
            ["command", "path", "query", "pattern"]
                .iter()
                .find_map(|name| {
                    args.get(*name)
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
        })
        .unwrap_or_default();

    let reported = tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        if let Some(open) = open.as_mut() {
            let _ = open
                .acted(injection, &call.name, &action, !failed)
                .await
                .inspect_err(|why| magi_model::noted!("turn: an outcome was refused: {why}"));
        }
    })
    .await;
    if reported.is_err() {
        magi_model::noted!("turn: an outcome did not land within {PATIENCE:?}");
    }
}
