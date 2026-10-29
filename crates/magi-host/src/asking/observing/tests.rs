#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::*;
use crate::asking::{Asker, Pending};
use crate::events::{Receiver, decode, enable_local};
use magi_proto::{Cursor, permit::Action};
use std::{sync::Arc, time::Duration};

fn outcomes(receiver: &Receiver) -> Vec<PermissionOutcome> {
    let mut outcomes = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        let event = decode(&bytes).expect("strict event");
        assert!(event.correlation.request_id.is_some());
        let Fact::Permission { outcome } = event.fact else {
            panic!("unexpected fact")
        };
        outcomes.push(outcome);
    }
    outcomes
}

#[test]
fn expiry_and_unwinding_are_not_denial_or_duplicate_completion() {
    if std::env::var_os("MAGI_EVENT_PERMISSION_PROBE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "asking::observing::tests::expiry_and_unwinding_are_not_denial_or_duplicate_completion"])
            .env("MAGI_EVENT_PERMISSION_PROBE", "1")
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
    let pending = Arc::new(Pending::new());
    let asker = Asker::new(
        pending.clone(),
        Box::new(|_| {}),
        Box::new(|| Cursor::ZERO),
        Box::new(|| true),
    );
    assert_eq!(
        asker.local(
            "private-tool",
            &Action::Read {
                path: "/private/path".into()
            },
            None,
            Duration::ZERO
        ),
        Decision::Deny
    );
    assert!(pending.open().is_empty());
    assert_eq!(
        outcomes(&receiver),
        [PermissionOutcome::Offered, PermissionOutcome::Expired]
    );
    let mut trace = Trace::new();
    trace.finish(PermissionOutcome::Allowed);
    trace.finish(PermissionOutcome::Denied);
    drop(trace);
    assert_eq!(outcomes(&receiver), [PermissionOutcome::Allowed]);
    drop(Trace::new());
    assert_eq!(outcomes(&receiver), [PermissionOutcome::Unknown]);
}
