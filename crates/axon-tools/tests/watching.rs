//! The tool-completion seam.
//!
//! axon reports that a tool ran and whether it worked. What anything does with that is not
//! axon's business — but *that* it is reported, on both completion paths and without being able
//! to affect the result, is.

use axon_tools::{Cancel, Ops, Output, Registry, Tool, Uncancelled, Watch};
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
            }
        }
    }
}

/// A watcher that writes down what it was told.
#[derive(Clone, Default)]
struct Noted(Rc<RefCell<Vec<(String, bool)>>>);

impl Watch for Noted {
    fn finished(&self, name: &str, _: &serde_json::Value, is_error: bool) {
        self.0.borrow_mut().push((name.to_owned(), is_error));
    }
}

fn ops() -> axon_tools::ops::Real {
    axon_tools::ops::Real::new(std::env::temp_dir())
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
        fn finished(&self, _: &str, _: &serde_json::Value, _: bool) {}
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
