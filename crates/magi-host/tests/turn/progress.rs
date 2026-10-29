use super::{backend, session};
use magi_proto::{Entry, ToolResult};
use magi_testkit::{
    Mind,
    mind::{call_lines, retrying_line, stop_line, text_line},
};
use magi_tools::{Output, Registry};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::Arc,
};
use tokio::sync::Mutex;

struct Spy {
    name: &'static str,
    sent: Rc<Cell<usize>>,
    collected: Rc<Cell<usize>>,
    replies: RefCell<VecDeque<Output>>,
    cancel: Option<magi_host::cancel::Cancel>,
}

impl magi_tools::Tool for Spy {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Scripted pending result"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    fn send(&self, _: &serde_json::Value, _: &dyn magi_tools::Ops) -> magi_tools::Sending {
        self.sent.set(self.sent.get() + 1);
        magi_tools::Sending::Sent
    }
    fn wait(&self, _: &dyn magi_tools::Cancel) -> Output {
        self.collected.set(self.collected.get() + 1);
        if let Some(cancel) = &self.cancel {
            cancel.request();
        }
        self.replies
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Output::error("unchanged failure"))
    }
    fn run(
        &self,
        _: &serde_json::Value,
        _: &dyn magi_tools::Ops,
        _: &dyn magi_tools::Cancel,
    ) -> Output {
        panic!("sent calls must be collected")
    }
}

fn spy(
    registry: &mut Registry,
    name: &'static str,
    replies: Vec<Output>,
    cancel: Option<magi_host::cancel::Cancel>,
) -> (Rc<Cell<usize>>, Rc<Cell<usize>>) {
    let sent = Rc::new(Cell::new(0));
    let collected = Rc::new(Cell::new(0));
    registry.register(Box::new(Spy {
        name,
        sent: sent.clone(),
        collected: collected.clone(),
        replies: RefCell::new(replies.into()),
        cancel,
    }));
    (sent, collected)
}

fn batch(calls: &[(&str, &str, &str)]) -> Vec<String> {
    let mut lines = Vec::new();
    for (id, name, args) in calls {
        lines.extend(call_lines(id, name, args).into_iter().take(2));
    }
    lines.push(magi_testkit::mind::stopped_line("tool_use"));
    lines
}

fn mind(name: &str, rounds: &[Vec<String>]) -> Mind {
    let refs: Vec<Vec<&str>> = rounds
        .iter()
        .map(|lines| lines.iter().map(String::as_str).collect())
        .collect();
    Mind::turns(name, &refs.iter().map(Vec::as_slice).collect::<Vec<_>>())
}

async fn run(session: &Mutex<magi_host::session::Session>, mind: &Mind, registry: &Registry) {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        magi_host::turn::run(
            session,
            &backend(mind),
            registry,
            &magi_tools::ops::Real::new(std::env::temp_dir()),
            &Arc::new(Mutex::new(None)),
        ),
    )
    .await
    .expect("bounded turn")
    .expect("turn");
}

fn results(entries: &[Entry]) -> Vec<(String, ToolResult)> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool { id, result, .. } => {
                Some((id.to_string(), result.clone().expect("every call paired")))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn repeated_failures_execute_twice_suppress_third_and_end_on_fourth_request() {
    let (session, _dir) = session("progress-repeated");
    let mind = mind(
        "progress-repeated",
        &[
            batch(&[("one", "spy", r#"{"a":1,"secret":"private"}"#)]),
            batch(&[("two", "spy", r#"{ "secret":"private", "a":1 }"#)]),
            batch(&[("three", "spy", r#"{"a":1,"secret":"private"}"#)]),
            batch(&[("four", "spy", r#"{"a":1,"secret":"private"}"#)]),
        ],
    );
    let mut registry = Registry::new();
    let (sent, collected) = spy(&mut registry, "spy", vec![], None);
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (2, 2, 4));
    assert!(mind.asks()[3].contains("Repeated-call guard: not executed"));
    let held = session.lock().await;
    let results = results(held.entries());
    assert_eq!(
        results
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three", "four"]
    );
    for (_, result) in &results[2..] {
        assert!(result.is_error && result.output.starts_with("Repeated-call guard:"));
        assert!(!result.output.contains("private"));
    }
    assert!(held.entries().iter().any(|entry| matches!(entry, Entry::Assistant {error: Some(error), ..} if error.starts_with("Repeated-call guard: stopped"))));
}

#[tokio::test]
async fn duplicate_batch_admission_preserves_unrelated_inflight_results() {
    let (session, _dir) = session("progress-batch");
    let mind = mind(
        "progress-batch",
        &[batch(&[
            ("one", "spy", "{}"),
            ("two", "spy", "{}"),
            ("three", "spy", "{}"),
            ("four", "spy", "{}"),
            ("late", "other", "{}"),
            ("five", "spy", "{}"),
        ])],
    );
    let mut registry = Registry::new();
    let (sent, collected) = spy(&mut registry, "spy", vec![], None);
    let (other_sent, other_collected) = spy(
        &mut registry,
        "other",
        vec![Output::ok("late result")],
        None,
    );
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (2, 2, 1));
    assert_eq!((other_sent.get(), other_collected.get()), (1, 1));
    let held = session.lock().await;
    let results = results(held.entries());
    assert_eq!(results.len(), 6);
    assert_eq!(results[4].0, "late");
    assert_eq!(results[4].1.output, "late result");
    assert!(!results[4].1.is_error);
}

#[tokio::test]
async fn successful_repeats_and_changed_failures_keep_running() {
    let (session, _dir) = session("progress-changed");
    let mut rounds: Vec<_> = (0..6)
        .map(|n| batch(&[(&n.to_string(), "spy", "{}")]))
        .collect();
    rounds.push(vec![text_line("done"), stop_line()]);
    let mind = mind("progress-changed", &rounds);
    let mut registry = Registry::new();
    let (sent, collected) = spy(
        &mut registry,
        "spy",
        vec![
            Output::error("first"),
            Output::error("second"),
            Output::error("third"),
            Output::ok("same"),
            Output::ok("same"),
            Output::ok("same"),
        ],
        None,
    );
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (6, 6, 7));
    assert_eq!(results(session.lock().await.entries()).len(), 6);
}

#[tokio::test]
async fn retry_frames_are_not_tool_completions_and_new_prompts_reset_guard() {
    let (session, _dir) = session("progress-retry");
    let mut retried = batch(&[("discarded", "spy", "{}")]);
    retried.pop();
    retried.push(retrying_line(1, 3, 0.0));
    retried.extend(batch(&[("actual", "spy", "{}")]));
    let mind = mind("progress-retry", &[retried]);
    let mut registry = Registry::new();
    let (sent, collected) = spy(&mut registry, "spy", vec![], None);
    for _ in 0..2 {
        run(&session, &mind, &registry).await;
    }
    assert_eq!((sent.get(), collected.get(), mind.asked()), (4, 4, 8));
    let held = session.lock().await;
    assert_eq!(results(held.entries()).len(), 8);
    assert!(results(held.entries()).iter().all(|(id, _)| id == "actual"));
}

#[tokio::test]
async fn cancellation_drains_sent_calls_and_never_prepares_deferred_duplicates() {
    let (session, _dir) = session("progress-cancel");
    let cancel = session.lock().await.cancel();
    let mind = mind(
        "progress-cancel",
        &[batch(&[
            ("one", "spy", "{}"),
            ("two", "spy", "{}"),
            ("late", "other", "{}"),
        ])],
    );
    let mut registry = Registry::new();
    let (sent, collected) = spy(&mut registry, "spy", vec![], Some(cancel));
    let (other_sent, other_collected) = spy(&mut registry, "other", vec![Output::ok("late")], None);
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (1, 1, 1));
    assert_eq!((other_sent.get(), other_collected.get()), (1, 1));
    let held = session.lock().await;
    let results = results(held.entries());
    assert_eq!(results.len(), 3);
    assert!(results[1].1.output.contains("cancelled"));
    assert_eq!(results[2].1.output, "late");
}

#[tokio::test]
async fn output_spill_paths_do_not_fabricate_changed_results() {
    let (session, _dir) = session("progress-spill");
    let mind = mind("progress-spill", &[batch(&[("same", "spy", "{}")])]);
    let mut registry = Registry::new();
    let output = Output::error("same failure\n".repeat(3_000));
    let (sent, collected) = spy(&mut registry, "spy", vec![output.clone(), output], None);
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (2, 2, 4));
    let held = session.lock().await;
    let results = results(held.entries());
    assert!(results[0].1.output.contains("cut from the middle"));
    assert!(results[2].1.output.starts_with("Repeated-call guard:"));
}

#[tokio::test]
async fn successful_duplicate_batch_and_alternating_failure_keys_have_defined_limits() {
    let (session, _dir) = session("progress-success-batch");
    let done = vec![text_line("done"), stop_line()];
    let mind = mind(
        "progress-success-batch",
        &[
            batch(&[
                ("one", "spy", "{}"),
                ("two", "spy", "{}"),
                ("three", "spy", "{}"),
                ("four", "spy", "{}"),
            ]),
            done,
        ],
    );
    let mut registry = Registry::new();
    let (sent, collected) = spy(&mut registry, "spy", vec![Output::ok(""); 4], None);
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (4, 4, 2));
    assert!(
        results(session.lock().await.entries())
            .iter()
            .all(|(_, result)| !result.is_error)
    );

    let (session, _dir) = super::session("progress-alternating");
    let mind = self::mind(
        "progress-alternating",
        &[batch(&[("a", "spy", "{}"), ("b", "other", "{}")])],
    );
    let mut registry = Registry::new();
    let (sent, collected) = spy(&mut registry, "spy", vec![], None);
    let (other_sent, other_collected) = spy(&mut registry, "other", vec![], None);
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (2, 2, 4));
    assert_eq!((other_sent.get(), other_collected.get()), (2, 2));
    assert_eq!(results(session.lock().await.entries()).len(), 8);
}

#[tokio::test]
async fn preparation_refusals_are_completed_failures_not_an_unbounded_loop() {
    for name in ["unknown", "spy"] {
        let tag = format!("progress-invalid-{name}");
        let (session, _dir) = session(&tag);
        let mind = mind(&tag, &[batch(&[("bad", name, "not JSON")])]);
        let mut registry = Registry::new();
        let (sent, collected) = spy(&mut registry, "spy", vec![], None);
        run(&session, &mind, &registry).await;
        assert_eq!((sent.get(), collected.get(), mind.asked()), (0, 0, 4));
        let held = session.lock().await;
        let results = results(held.entries());
        assert_eq!(results.len(), 4);
        assert!(results[2].1.output.starts_with("Repeated-call guard:"));
    }
}

#[tokio::test]
async fn failed_attachment_validation_counts_as_failure_before_text_is_bounded() {
    let (session, _dir) = session("progress-attachments");
    let mind = mind("progress-attachments", &[batch(&[("same", "spy", "{}")])]);
    let mut registry = Registry::new();
    let mut output = Output::ok("images");
    output.images = vec![
        magi_model::Image {
            data: "iVBORw0K".into(),
            media_type: "image/png".into()
        };
        5
    ];
    let (sent, collected) = spy(&mut registry, "spy", vec![output.clone(), output], None);
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (2, 2, 4));
    let held = session.lock().await;
    let results = results(held.entries());
    assert!(results[0].1.is_error && results[0].1.output.contains("Image attachments omitted"));
    assert!(results[2].1.output.starts_with("Repeated-call guard:"));
}

#[tokio::test]
async fn successful_polling_still_obeys_the_coarse_round_limit() {
    let (session, _dir) = session("progress-round-limit");
    let mind = mind("progress-round-limit", &[batch(&[("poll", "spy", "{}")])]);
    let mut registry = Registry::new();
    let (sent, collected) = spy(&mut registry, "spy", vec![Output::ok("pending"); 200], None);
    run(&session, &mind, &registry).await;
    assert_eq!((sent.get(), collected.get(), mind.asked()), (200, 200, 200));
    let held = session.lock().await;
    assert!(
        results(held.entries())
            .iter()
            .all(|(_, result)| !result.is_error)
    );
    assert!(held.entries().iter().any(|entry| matches!(entry, Entry::Assistant {error: Some(error), ..} if error.contains("stopped after 200 rounds"))));
}
