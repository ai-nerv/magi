#![allow(clippy::expect_used)]

use super::*;
use crate::events::collection::bootstrap::tests::native::isolate;

#[test]
fn tree_failure_and_total_overrun_remain_visible_after_scope_teardown() {
    const CASE: &str = "events::collection::runtime::tests::tree_failure_and_total_overrun_remain_visible_after_scope_teardown";
    if isolate(CASE) {
        return;
    }
    assert_eq!(
        serde_json::to_value(status()).expect("disabled"),
        serde_json::json!({"state":"disabled"})
    );
    let scope = Scope::start().expect("scope");
    assert!(!status().worker_failed);
    WATCH.get().expect("installed watch").fail();
    OVERRUN.store(true, Ordering::Release);
    let live = status();
    assert!(live.worker_failed && live.shutdown_overrun);
    let summary = live.summary.expect("live summary");
    assert!(summary.worker_failed && summary.shutdown_overrun && summary.incomplete());
    drop(scope);
    let closed = status();
    assert!(closed.worker_failed && closed.shutdown_overrun);
    let summary = closed.summary.expect("closed summary");
    assert!(
        summary.closed && summary.worker_failed && summary.shutdown_overrun && summary.incomplete()
    );
}
