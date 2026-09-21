use super::{Live, answered, backend, live};
use magi_host::helping::{Job, work};
use magi_testkit::mind::{
    call_lines, failed_line, retrying_line, stop_line, stopped_line, text_line,
};
use magi_testkit::{Mind, memory::Serving};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

async fn queued(live: &Live) -> Job {
    let mut family =
        magi_ipc::family::Family::dial(live.serving.as_ref().expect("server").socket())
            .await
            .expect("dial");
    family.call("observe", vec![json!(live.id.to_string()), json!({
        "cursor": 1, "role": "user", "kind": "user", "text": "The parser is written in Rust.",
    })]).await.expect("observe");
    let jobs = family
        .call(
            "jobs",
            vec![
                json!(live.id.to_string()),
                json!({"helpers":["notes","curate"]}),
            ],
        )
        .await
        .expect("jobs");
    serde_json::from_value(
        jobs.into_iter()
            .find(|job| job["kind"] == "extract")
            .expect("extract"),
    )
    .expect("job")
}

fn ops(title: &str) -> String {
    json!({"ops":[{"op":"add", "title":title, "text":"The parser is written in Rust.",
        "description":"Parser language", "pinned":false}]})
    .to_string()
}

async fn perform(live: &Live, job: Job, mind: &Mind) -> u64 {
    let mut backend = backend(mind, 200_000);
    backend
        .helpers
        .roles
        .insert("memory".into(), "fake/one".into());
    let (events, _heard) = tokio::sync::broadcast::channel(8);
    let spent = AtomicU64::new(0);
    work(&[job], &backend, &live.scribe, &events, &spent)
        .await
        .expect("persist helper outcome");
    spent.load(Ordering::Relaxed)
}

#[tokio::test]
async fn only_the_final_helper_attempt_reaches_durable_memory() {
    for mode in ["text", "tool"] {
        let Some(mut live) = live(&format!("hr-{mode}")).await else {
            return;
        };
        let job = queued(&live).await;
        let final_lines = if mode == "tool" {
            call_lines("final", "schema", &ops("Parser"))
        } else {
            vec![text_line(&ops("Parser")), stop_line()]
        };
        let abandoned = call_lines("old", "schema", r#"{"ops":["#);
        let mind = Mind::saying(
            &format!("hr-{mode}"),
            &[
                &text_line(&ops("Abandoned")),
                &abandoned[0],
                &abandoned[1],
                &retrying_line(1, 3, 0.0),
                &final_lines.join("\n"),
            ],
        );
        perform(&live, job, &mind).await;
        let changes = answered(&live, "changes", json!({})).await;
        assert_eq!(changes.as_array().expect("changes").len(), 1, "{changes}");
        assert_eq!(changes[0]["after"]["title"], "Parser");
        assert_eq!(changes[0]["state"], "applied");

        drop(live.serving.take());
        let serving = Serving::start(&live._dir, &live.id.to_string())
            .await
            .expect("reopen owned store");
        let mut family = magi_ipc::family::Family::dial(serving.socket())
            .await
            .expect("dial reopened store");
        let notes = family
            .call("notes", vec![json!(live.id.to_string())])
            .await
            .expect("notes");
        assert_eq!(notes[0]["deferred"].as_array().expect("notes").len(), 1);
        assert_eq!(notes[0]["deferred"][0]["title"], "Parser");
        assert!(!json!(notes).to_string().contains("Abandoned"));
    }
}

#[tokio::test]
async fn failed_helpers_report_failure_without_applying_their_partial_memory() {
    for (name, terminal) in [
        ("fail", failed_line("attempts exhausted", "transport")),
        ("length", stopped_line("length")),
    ] {
        let Some(live) = live(&format!("hr-{name}")).await else {
            return;
        };
        let job = queued(&live).await;
        let id = job.id.clone();
        let mind = Mind::saying(
            &format!("hr-{name}"),
            &[
                &text_line("abandoned"),
                &retrying_line(1, 2, 0.0),
                &text_line(&ops("Must not land")),
                &terminal,
            ],
        );
        perform(&live, job, &mind).await;
        let jobs = live
            .scribe
            .lock()
            .await
            .as_mut()
            .expect("scribe")
            .jobs()
            .await
            .expect("retry jobs");
        let retry: Job = serde_json::from_value(
            jobs.into_iter()
                .find(|job| job["id"] == id)
                .expect("failed job requeued"),
        )
        .expect("retry");
        perform(&live, retry, &mind).await;
        assert!(
            live.scribe
                .lock()
                .await
                .as_mut()
                .expect("scribe")
                .jobs()
                .await
                .expect("jobs")
                .is_empty()
        );
        let changes = answered(&live, "changes", json!({})).await;
        assert_eq!(changes, json!([]));
        let notes = answered(&live, "notes", json!({})).await;
        assert_eq!(notes[0]["pinned"], json!([]));
        assert_eq!(notes[0]["deferred"], json!([]));
    }
}
