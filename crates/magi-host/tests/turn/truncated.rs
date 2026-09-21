use super::{backend, session};
use magi_proto::Entry;
use magi_testkit::Mind;
use magi_testkit::mind::{call_lines, stopped_line};
use std::{cell::Cell, rc::Rc};

struct Spy(Rc<Cell<usize>>);

impl magi_tools::Tool for Spy {
    fn name(&self) -> &str {
        "spy"
    }
    fn description(&self) -> &str {
        "Count calls"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    fn run(
        &self,
        _: &serde_json::Value,
        _: &dyn magi_tools::Ops,
        _: &dyn magi_tools::Cancel,
    ) -> magi_tools::Output {
        self.0.set(self.0.get() + 1);
        magi_tools::Output::ok("executed")
    }
}

#[tokio::test]
async fn truncated_calls_are_journalled_with_failed_results_and_never_executed() {
    let (session, _dir) = session("truncated");
    let complete = call_lines("complete", "spy", "{}");
    let partial = call_lines("partial", "spy", r#"{"path":"#);
    let mind = Mind::saying(
        "turn-truncated",
        &[
            &complete[0],
            &complete[1],
            &partial[0],
            &partial[1],
            &stopped_line("length"),
        ],
    );
    let called = Rc::new(Cell::new(0));
    let mut registry = magi_tools::Registry::new();
    registry.register(Box::new(Spy(Rc::clone(&called))));
    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    let scribe = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    magi_host::turn::run(&session, &backend(&mind), &registry, &ops, &scribe)
        .await
        .expect("turn");
    assert_eq!(called.get(), 0);
    let mut held = session.lock().await;
    let calls: Vec<_> = held
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool {
                id,
                name,
                args,
                result,
                ..
            } => Some((id.to_string(), name.clone(), args.clone(), result.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2, "every attempted call needs a failed result");
    assert_eq!(calls[0].0, "complete");
    assert_eq!(calls[0].2, "{}");
    assert_eq!(calls[1].0, "partial");
    assert_eq!(calls[1].2, r#"{"path":"#);
    for (_, name, _, result) in &calls {
        assert_eq!(name, "spy");
        let result = result.as_ref().expect("failed result");
        assert!(result.is_error);
        assert!(result.output.contains("truncated"));
    }
    assert_eq!(
        held.take_pending()
            .iter()
            .filter(|(_, e)| matches!(e, Entry::Tool { .. }))
            .count(),
        2
    );
}
