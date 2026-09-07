//! The tool-completion seam.
//!
//! magi reports that a tool ran and whether it worked. What anything does with that is not
//! magi's business — but *that* it is reported, on both completion paths and without being able
//! to affect the result, is.

use magi_tools::{Cancel, Ops, Output, Registry, Tool, Uncancelled, Watch};
use std::cell::RefCell;
use std::rc::Rc;

/// A tool that succeeds or fails on command.
struct Fake {
    name: &'static str,
    fails: bool,
}

impl Tool for Fake {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "a tool"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    fn run(&self, _: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
        if self.fails {
            Output::error("no")
        } else {
            Output {
                content: "ok".to_owned(),
                is_error: false,
                shown: None,
            }
        }
    }
}

/// A watcher that writes down what it was told.
#[derive(Clone, Default)]
struct Noted(Rc<RefCell<Vec<(String, bool)>>>);

impl Watch for Noted {
    fn saw(&self, event: &magi_tools::Event<'_>) {
        if let magi_tools::Event::Tool { name, is_error, .. } = event {
            self.0.borrow_mut().push(((*name).to_owned(), *is_error));
        }
    }
}

fn ops() -> magi_tools::ops::Real {
    magi_tools::ops::Real::new(std::env::temp_dir())
}

#[test]
fn a_watcher_is_told_what_ran_and_how_it_went() {
    let mut registry = Registry::new();
    registry.register(Box::new(Fake {
        name: "build",
        fails: false,
    }));
    let noted = Noted::default();
    registry.watch(Box::new(noted.clone()));

    let _ = registry.call("build", &serde_json::json!({}), &ops(), &Uncancelled);

    assert_eq!(noted.0.borrow().as_slice(), &[("build".to_owned(), false)]);
}

#[test]
fn a_failure_is_reported_as_one() {
    // Half the signal. A seam that only reported successes would be worse than none — anything
    // reading it would conclude that everything works.
    let mut registry = Registry::new();
    registry.register(Box::new(Fake {
        name: "build",
        fails: true,
    }));
    let noted = Noted::default();
    registry.watch(Box::new(noted.clone()));

    let _ = registry.call("build", &serde_json::json!({}), &ops(), &Uncancelled);

    assert_eq!(noted.0.borrow().as_slice(), &[("build".to_owned(), true)]);
}

#[test]
fn a_call_that_never_reached_a_tool_reports_nothing() {
    // A name that does not exist is not a tool that ran. Saying otherwise would put phantom
    // actions in whatever is counting.
    let mut registry = Registry::new();
    let noted = Noted::default();
    registry.watch(Box::new(noted.clone()));

    let _ = registry.call("nothing", &serde_json::json!({}), &ops(), &Uncancelled);

    assert!(noted.0.borrow().is_empty());
}

#[test]
fn a_watcher_cannot_change_the_result() {
    // It is told after the fact and its answer is ignored, so observing a call is never a way
    // of breaking one.
    struct Meddler;
    impl Watch for Meddler {
        fn saw(&self, _: &magi_tools::Event<'_>) {}
    }

    let mut registry = Registry::new();
    registry.register(Box::new(Fake {
        name: "build",
        fails: false,
    }));
    registry.watch(Box::new(Meddler));

    let out = registry.call("build", &serde_json::json!({}), &ops(), &Uncancelled);
    assert_eq!(out.content, "ok");
    assert!(!out.is_error);
}

#[test]
fn every_watcher_is_told() {
    let mut registry = Registry::new();
    registry.register(Box::new(Fake {
        name: "build",
        fails: false,
    }));
    let one = Noted::default();
    let two = Noted::default();
    registry.watch(Box::new(one.clone()));
    registry.watch(Box::new(two.clone()));

    let _ = registry.call("build", &serde_json::json!({}), &ops(), &Uncancelled);

    assert_eq!(one.0.borrow().len(), 1);
    assert_eq!(two.0.borrow().len(), 1);
}

#[test]
fn a_registry_with_nothing_watching_still_works() {
    // The ordinary case. Nobody has to install a watcher, and the cost of not having one is an
    // empty loop.
    let mut registry = Registry::new();
    registry.register(Box::new(Fake {
        name: "build",
        fails: false,
    }));
    let out = registry.call("build", &serde_json::json!({}), &ops(), &Uncancelled);
    assert_eq!(out.content, "ok");
}

/// A watcher that writes down the name of everything it is told.
#[derive(Clone, Default)]
struct Everything(Rc<RefCell<Vec<String>>>);

impl Watch for Everything {
    fn saw(&self, event: &magi_tools::Event<'_>) {
        self.0.borrow_mut().push(event.kind().to_owned());
    }
}

#[test]
fn one_registration_hears_about_more_than_tools() {
    // The seam was a callback with one caller: a tool finished, and nothing else in a session
    // was observable from outside at all. A watcher registered once now hears everything,
    // rather than needing a second registration under a second name per kind of event.
    let seen = Everything::default();
    let mut registry = Registry::new();
    registry.watch(Box::new(seen.clone()));
    registry.saw(&magi_tools::Event::TurnBegan { model: "m" });
    registry.saw(&magi_tools::Event::Compacted {
        dropped: 2,
        kept: 8,
    });
    assert_eq!(
        seen.0.borrow().as_slice(),
        ["turn.began", "context.compacted"]
    );
}

#[test]
fn a_permission_is_written_down_where_it_is_decided_and_read_where_the_watchers_are() {
    // The one event that cannot be delivered where it happens: `Ops` is `Send + Sync` and a
    // watcher is neither. What the gate can do is write it down; what the turn loop does is
    // read it out. Draining is destructive, so the same question is not reported twice.
    let waiting = magi_tools::watching::Pending::new();
    waiting.note(magi_tools::watching::Noted {
        verb: "run".to_owned(),
        about: "git status".to_owned(),
        allowed: true,
    });
    let taken = waiting.drain();
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].verb, "run");
    assert!(taken[0].allowed);
    assert!(waiting.drain().is_empty(), "taken once, not every round");
}

#[test]
fn ops_with_no_gate_notices_nothing() {
    // Every `Ops` answers this; only a gated one has anything to say. A double in a tool test
    // should not have to know that permissions exist.
    assert!(ops().noticed().is_empty());
}

#[test]
fn a_gated_ops_writes_down_the_question_and_the_answer() {
    // The audit trail that did not exist: a permission was put to somebody, answered, written
    // into the ledger, and then the fact that it had been asked at all was gone.
    struct Denies;
    impl magi_tools::approve::Approver for Denies {
        fn ask(
            &self,
            _tool: &str,
            _action: &magi_proto::permit::Action,
        ) -> magi_proto::permit::Decision {
            magi_proto::permit::Decision::Deny
        }
    }
    let ops = magi_tools::ops::Real::gated(
        std::env::temp_dir(),
        magi_tools::permit::Ledger::default(),
        std::sync::Arc::new(Denies),
    );
    let refused = ops.allow(
        "bash",
        &magi_proto::permit::Action::Run {
            command: "git status".to_owned(),
            program: "git".to_owned(),
        },
    );
    assert!(refused.is_err(), "the approver said no");

    let noticed = ops.noticed();
    assert_eq!(noticed.len(), 1, "{noticed:?}");
    assert_eq!(noticed[0].verb, "run");
    assert!(!noticed[0].allowed, "and it says which way it went");
}
