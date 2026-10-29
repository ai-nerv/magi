use super::*;
use crate::{
    approve::{Approver, Authorization},
    permit::{Action, Decision, Grant, Ledger, Lifetime, Scope},
};
use std::{cell::RefCell, sync::Arc};

thread_local! {
    static SEEN: RefCell<Vec<Authorization>> = const { RefCell::new(Vec::new()) };
}

fn observe(outcome: Authorization) {
    SEEN.with_borrow_mut(|seen| seen.push(outcome));
}

fn take() -> Vec<Authorization> {
    SEEN.with_borrow_mut(std::mem::take)
}

fn action() -> Action {
    Action::Read {
        path: "/private/read".into(),
    }
}

struct Answer {
    decision: Decision,
    override_grant: bool,
}

impl Approver for Answer {
    fn ask(&self, _: &str, _: &Action) -> Decision {
        self.decision.clone()
    }
    fn overrides(&self, _: &Action) -> bool {
        self.override_grant
    }
    fn why(&self, _: &Action) -> Option<String> {
        Some("private refusal reason".into())
    }
}

fn gated(decision: Decision, standing: bool, override_grant: bool) -> Real {
    Real::gated(
        PathBuf::from("/private"),
        Ledger::with(if standing {
            vec![Grant {
                verb: "read".into(),
                scope: Scope::Anything,
            }]
        } else {
            vec![]
        }),
        Arc::new(Answer {
            decision,
            override_grant,
        }),
    )
}

fn allow(scope: Scope) -> Decision {
    Decision::Allow {
        scope,
        lifetime: Lifetime::Session,
    }
}

#[test]
fn absent_observer_leaves_native_gate_results_unchanged() {
    let _ = take();
    for ops in [
        Real::new("/private".into()),
        Real::confined("/private".into()),
    ] {
        assert!(ops.allow("private-tool", &action()).is_ok());
    }
    assert!(
        gated(Decision::Deny, false, false)
            .allow("private-tool", &action())
            .is_err()
    );
    assert!(
        gated(Decision::Deny, true, false)
            .allow("private-tool", &action())
            .is_ok()
    );
    assert!(take().is_empty());
}

#[test]
fn ungated_and_confined_constructors_observe_without_notices_or_grants() {
    let _ = take();
    for ops in [
        Real::new("/private".into()),
        Real::confined("/private".into()),
    ] {
        let ops = ops.observing(observe);
        assert!(ops.allow("private-tool", &action()).is_ok());
        assert!(ops.grants().is_empty());
        assert!(ops.noticed().is_empty());
    }
    assert_eq!(take(), [Authorization::Ungated; 2]);
}

#[test]
fn remembered_grant_is_distinct_from_an_approver_answer() {
    let _ = take();
    let ops = gated(
        allow(Scope::Directory {
            path: "/private".into(),
        }),
        false,
        false,
    )
    .observing(observe);
    for _ in 0..2 {
        ops.allow("private-tool", &action()).expect("allowed");
    }
    assert_eq!(
        take(),
        [Authorization::ApproverAllowed, Authorization::StandingGrant]
    );
    assert_eq!(
        ops.grants(),
        [Grant {
            verb: "read".into(),
            scope: Scope::Directory {
                path: "/private".into()
            }
        }]
    );
    let notices = ops.noticed();
    assert_eq!(notices.len(), 1);
    let note = notices.first().expect("notice");
    assert_eq!(note.verb, "read");
    assert_eq!(note.about, "/private/read");
    assert!(note.allowed);
}

#[test]
fn once_answers_do_not_become_standing_grants() {
    let _ = take();
    let ops = gated(allow(Scope::Once), false, false).observing(observe);
    for _ in 0..2 {
        ops.allow("private-tool", &action()).expect("allowed");
    }
    assert_eq!(take(), [Authorization::ApproverAllowed; 2]);
    assert!(ops.grants().is_empty());
    assert_eq!(ops.noticed().len(), 2);
}

#[test]
fn override_refusal_preserves_exact_error_and_notices() {
    let _ = take();
    let expected = gated(Decision::Deny, true, true).allow("private-tool", &action());
    let ops = gated(Decision::Deny, true, true).observing(observe);
    assert_eq!(ops.allow("private-tool", &action()), expected);
    assert_eq!(take(), [Authorization::ApproverDenied]);
    let notes = ops.noticed();
    assert_eq!(notes.len(), 1);
    assert!(!notes.first().expect("notice").allowed);
    assert_eq!(ops.grants().len(), 1);
}

#[test]
fn observer_unwind_does_not_change_authority_or_native_results() {
    fn panic_observer(_: Authorization) {
        panic!("observer failure");
    }
    for (decision, standing, override_grant) in [
        (Decision::Deny, false, false),
        (Decision::Deny, true, false),
        (Decision::Deny, true, true),
        (allow(Scope::Once), false, false),
        (allow(Scope::Anything), false, false),
    ] {
        let plain = gated(decision.clone(), standing, override_grant);
        let observed = gated(decision, standing, override_grant).observing(panic_observer);
        for _ in 0..2 {
            assert_eq!(
                observed.allow("private-tool", &action()),
                plain.allow("private-tool", &action())
            );
            assert_eq!(observed.grants(), plain.grants());
            let actual = observed.noticed();
            let expected = plain.noticed();
            assert_eq!(actual.len(), expected.len());
            for (a, b) in actual.iter().zip(expected) {
                assert_eq!(
                    (&a.verb, &a.about, a.allowed),
                    (&b.verb, &b.about, b.allowed)
                );
            }
        }
    }
    assert!(
        Real::new("/private".into())
            .observing(panic_observer)
            .allow("private-tool", &action())
            .is_ok()
    );
}

#[test]
fn an_approver_unwind_emits_no_completed_authorization() {
    struct Broken;
    impl Approver for Broken {
        fn ask(&self, _: &str, _: &Action) -> Decision {
            panic!("approver failure");
        }
    }
    let _ = take();
    let ops = Real::gated("/private".into(), Ledger::new(), Arc::new(Broken)).observing(observe);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || ops.allow("private-tool", &action())
        ))
        .is_err()
    );
    assert!(take().is_empty());
    assert!(ops.noticed().is_empty());
    assert!(ops.grants().is_empty());
}
