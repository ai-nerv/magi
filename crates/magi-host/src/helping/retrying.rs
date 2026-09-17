use super::tests::{backend, helpers};
use crate::helping::{Job, run};
use magi_testkit::Mind;
use magi_testkit::mind::{
    call_lines, failed_line, retrying_line, stop_line, stopped_line, text_line,
};
use serde_json::json;

fn job() -> Job {
    Job {
        role: "memory".into(),
        schema: Some(json!({"type": "object"})),
        ..Job::default()
    }
}

fn spent(input: u64, cost: u64) -> String {
    json!({"event": "spent", "usage": {
        "input": input, "output": 2, "cache_read": 3,
        "cache_write": 4, "cost_micros": cost,
    }})
    .to_string()
}

#[tokio::test]
async fn retries_discard_abandoned_text_and_count_each_attempt_once() {
    let mind = Mind::saying(
        "helper-retry-text",
        &[
            &text_line("discard me"),
            &spent(1, 2),
            &spent(10, 20),
            &retrying_line(1, 4, 0.0),
            &text_line("discard me too"),
            &spent(30, 40),
            &retrying_line(2, 4, 0.0),
            &text_line(r#"{"ops":[]}"#),
            &spent(50, 60),
            &stop_line(),
        ],
    );
    let answer = run(&job(), &backend(&mind, helpers()))
        .await
        .expect("answer");
    assert_eq!(answer.text, r#"{"ops":[]}"#);
    assert_eq!(answer.usage.input, 90);
    assert_eq!(answer.usage.output, 6);
    assert_eq!(answer.usage.cache_read, 9);
    assert_eq!(answer.usage.cache_write, 12);
    assert_eq!(answer.usage.cost_micros, 120);
}

#[tokio::test]
async fn retries_discard_abandoned_tool_arguments_in_either_answer_mode() {
    let abandoned = call_lines("old", "schema", r#"{"ops":["#);
    let final_call = call_lines("new", "schema", r#"{"ops":[]}"#);
    for (mode, final_lines) in [
        ("tool", final_call),
        ("text", vec![text_line(r#"{"ops":[]}"#), stop_line()]),
    ] {
        let mind = Mind::saying(
            &format!("helper-retry-args-to-{mode}"),
            &[
                &abandoned[0],
                &abandoned[1],
                &retrying_line(1, 3, 0.0),
                &final_lines.join("\n"),
            ],
        );
        let answer = run(&job(), &backend(&mind, helpers()))
            .await
            .expect("answer");
        assert_eq!(answer.text, r#"{"ops":[]}"#);
    }
}

#[tokio::test]
async fn retry_at_eof_cannot_reuse_an_earlier_terminal_or_answer() {
    let mind = Mind::saying(
        "helper-retry-eof",
        &[
            &text_line("abandoned"),
            &stop_line(),
            &retrying_line(1, 3, 0.0),
        ],
    );
    assert!(run(&job(), &backend(&mind, helpers())).await.is_err());
}

#[tokio::test]
async fn failed_and_truncated_helpers_do_not_succeed_with_partial_output() {
    for (name, terminal) in [
        ("failed", failed_line("all attempts failed", "transport")),
        ("length", stopped_line("length")),
        ("aborted", stopped_line("aborted")),
        ("error", stopped_line("error")),
    ] {
        let mind = Mind::saying(
            &format!("helper-{name}"),
            &[
                &text_line("discard"),
                &retrying_line(1, 2, 0.0),
                &text_line(r#"{"ops":[]}"#),
                &terminal,
            ],
        );
        assert!(
            run(&job(), &backend(&mind, helpers())).await.is_err(),
            "{name}"
        );
    }
}

#[tokio::test]
async fn failed_attempt_usage_is_reported_and_stops_the_next_budgeted_job() {
    let mind = Mind::saying(
        "helper-failed-spend",
        &[
            &spent(10, 20),
            &retrying_line(1, 2, 0.0),
            &spent(30, 40),
            &failed_line("all attempts failed", "transport"),
        ],
    );
    let mut configured = helpers();
    configured.per_prompt_micros = Some(60);
    let (events, mut heard) = tokio::sync::broadcast::channel(8);
    let budget = std::sync::atomic::AtomicU64::new(0);
    super::work(
        &[job(), job()],
        &backend(&mind, configured),
        &std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        &events,
        &budget,
    )
    .await
    .expect("helper work settled");
    assert_eq!(mind.asked(), 1);
    assert_eq!(budget.load(std::sync::atomic::Ordering::Relaxed), 60);
    let magi_proto::HarnessEvent::HelperSpent { usage, .. } =
        heard.try_recv().expect("reported spend")
    else {
        panic!("spend event")
    };
    assert_eq!(usage.input, 40);
    assert_eq!(usage.cost_micros, 60);
    assert!(heard.try_recv().is_err());
}

#[tokio::test]
async fn malformed_and_multiple_tool_answers_are_failures() {
    for (name, text) in [("malformed", r#"{"ops":["#), ("concatenated", "{}{}")] {
        let mind = Mind::answering(&format!("helper-{name}"), text);
        assert!(run(&job(), &backend(&mind, helpers())).await.is_err());
    }
    let first = call_lines("first", "schema", "{}");
    let second = call_lines("second", "schema", "{}");
    let mind = Mind::saying(
        "helper-two-calls",
        &[&first[0], &first[1], &second.join("\n")],
    );
    assert!(run(&job(), &backend(&mind, helpers())).await.is_err());
}

#[cfg(target_os = "linux")]
async fn reaped(pid: u32) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("helper process reaped");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn timeout_keeps_reported_cost_and_reaps_the_provider() {
    let mind = Mind::stalling("helper-timeout", &[&text_line("partial"), &spent(10, 20)]);
    let mut job = job();
    job.timeout_ms = Some(500);
    let failure = super::run_accounted(&job, &backend(&mind, helpers()))
        .await
        .expect_err("timeout");
    assert!(failure.message.contains("did not answer within"));
    assert_eq!(failure.usage.cost_micros, 20);
    reaped(mind.stalled_pid().expect("provider started")).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancellation_drops_the_attempt_and_reaps_the_provider() {
    let mind = Mind::stalling(
        "helper-cancel",
        &[
            &text_line("abandoned"),
            &retrying_line(1, 3, 0.0),
            &text_line("partial"),
        ],
    );
    let backend = backend(&mind, helpers());
    let task = tokio::spawn(async move { run(&job(), &backend).await });
    let pid = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some(pid) = mind.stalled_pid() {
                break pid;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("provider started");
    task.abort();
    assert!(task.await.expect_err("cancelled task").is_cancelled());
    reaped(pid).await;
}
