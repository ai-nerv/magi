//! What a watcher is told when a call finishes, on both completion paths.

// A private import is visible to descendant modules, so the module below keeps saying
// `use super::*` and meaning what it meant in `registry.rs`.
use super::*;

#[cfg(test)]
mod watching_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A tool that reports itself as sent, like every peer does.
    struct Peer;

    impl Tool for Peer {
        fn name(&self) -> &str {
            "peer"
        }
        fn description(&self) -> &str {
            "a tool that is sent and waited for"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": { "path": { "type": "string" } } })
        }
        fn run(&self, _a: &serde_json::Value, _o: &dyn Ops, _c: &dyn Cancel) -> Output {
            Output::ok("ran inline")
        }
        fn send(&self, _a: &serde_json::Value, _o: &dyn Ops) -> Sending {
            Sending::Sent
        }
        fn wait(&self, _c: &dyn Cancel) -> Output {
            Output::ok("collected")
        }
    }

    /// An `Ops` that refuses everything: this test never reaches the filesystem.
    struct Nowhere;

    impl Ops for Nowhere {
        fn cwd(&self) -> std::path::PathBuf {
            std::path::PathBuf::from(".")
        }
        fn read(&self, _path: &std::path::Path) -> Result<String, String> {
            Err("no".to_owned())
        }
        fn write(&self, _path: &std::path::Path, _contents: &str) -> Result<(), String> {
            Err("no".to_owned())
        }
    }

    /// Remembers what it was told, which is the whole point of a watcher.
    struct Noted(Rc<RefCell<Vec<serde_json::Value>>>);

    impl Watch for Noted {
        fn saw(&self, event: &Event<'_>) {
            if let Event::Tool { arguments, .. } = event {
                self.0.borrow_mut().push((*arguments).clone());
            }
        }
    }

    #[test]
    fn a_watcher_is_told_what_a_sent_call_ran_with() {
        // `State::Sent` carried nothing, so `finish` reported `null` arguments for every peer tool,
        // `shell` included.
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut registry = Registry::new();
        registry.register(Box::new(Peer));
        registry.watch(Box::new(Noted(Rc::clone(&seen))));

        let ops = Nowhere;
        let cancel = crate::cancel::Uncancelled;
        // The registry is handed the argument text as the model streamed it, and parses it at the
        // one funnel every transport crosses.
        let asked = r#"{"path":"src/main.rs"}"#;
        let prepared = registry.prepare("peer", asked, &ops);
        assert!(prepared.in_flight(), "a peer tool is sent, not run inline");
        let _ = registry.finish(prepared, &ops, &cancel);

        let seen = seen.borrow();
        assert_eq!(seen.len(), 1, "one call, one telling");
        assert_eq!(
            seen[0]["path"],
            serde_json::json!("src/main.rs"),
            "the watcher was told what the call ran with, not null: {:?}",
            seen[0]
        );
    }
}
