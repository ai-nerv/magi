use super::{backend, session, turn};
use magi_host::session::Session;
use magi_proto::{Entry, HarnessEvent, MessageId};
use magi_testkit::{
    Mind,
    mind::{call_lines, stop_line, text_line},
};
use std::sync::Arc;
use tokio::sync::Mutex;

fn later(id: &str) -> Entry {
    Entry::User {
        id: MessageId::new(id),
        text: id.into(),
        aside: String::new(),
    }
}

#[tokio::test]
async fn streaming_owns_its_entry_even_when_another_entry_arrives_later() {
    let (session, _dir) = session("owned-stream");
    let mind = Mind::controlled("owned-stream");
    let backend = backend(&mind);
    let mut events = session.lock().await.subscribe();
    let inject = async {
        loop {
            if matches!(events.recv().await.expect("stream event"), HarnessEvent::AssistantDelta { text, .. } if text == "part-0")
            {
                session
                    .lock()
                    .await
                    .commit(later("later-user"))
                    .expect("later entry");
                mind.release(0);
                return;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::join!(turn(&session, &backend), inject);
    })
    .await
    .expect("stream completed");
    let held = session.lock().await;
    assert_eq!(held.entries().len(), 2);
    assert!(matches!(&held.entries()[0], Entry::Assistant { text, .. } if text == "part-0-done"));
    assert_eq!(held.entries()[1], later("later-user"));
}

struct Appending(Arc<Mutex<Session>>);

impl magi_tools::Tool for Appending {
    fn name(&self) -> &str {
        "append"
    }
    fn description(&self) -> &str {
        "Append a fixture entry"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    fn run(
        &self,
        args: &serde_json::Value,
        _: &dyn magi_tools::Ops,
        _: &dyn magi_tools::Cancel,
    ) -> magi_tools::Output {
        let text = args["note"].as_str().expect("fixture note");
        self.0
            .try_lock()
            .expect("tool does not hold session lock")
            .commit(later(text))
            .expect("fixture entry committed");
        magi_tools::Output::ok(text)
    }
}

#[tokio::test]
async fn tool_results_amend_their_own_calls_after_later_entries_arrive() {
    let (session, _dir) = session("owned-tools");
    let session = Arc::new(session);
    let first = call_lines("first-call", "append", r#"{"note":"first-note"}"#);
    let second = call_lines("second-call", "append", r#"{"note":"second-note"}"#);
    let mind = Mind::turns(
        "owned-tools",
        &[
            &[&first[0], &first[1], &second[0], &second[1], &second[2]],
            &[&text_line("finished"), &stop_line()],
        ],
    );
    let mut registry = magi_tools::Registry::new();
    registry.register(Box::new(Appending(Arc::clone(&session))));
    let scribe = Arc::new(Mutex::new(None));
    magi_host::turn::run(
        &session,
        &backend(&mind),
        &registry,
        &magi_tools::ops::Real::new(std::env::temp_dir()),
        &scribe,
    )
    .await
    .expect("tool turn");
    let held = session.lock().await;
    assert_eq!(held.entries().len(), 6);
    for (index, id, output) in [
        (1, "first-call", "first-note"),
        (2, "second-call", "second-note"),
    ] {
        assert!(
            matches!(&held.entries()[index], Entry::Tool { id: actual, result: Some(result), .. }
            if actual.as_str() == id && result.output == output && !result.is_error)
        );
    }
    assert_eq!(held.entries()[3], later("first-note"));
    assert_eq!(held.entries()[4], later("second-note"));
    assert!(matches!(&held.entries()[5], Entry::Assistant { text, .. } if text == "finished"));
    assert_eq!(mind.asked(), 2);
}
