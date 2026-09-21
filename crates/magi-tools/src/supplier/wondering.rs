//! A tool that stops to ask magi something, and goes on with the answer.

use super::*;
use crate::holding::Answers;
use magi_proto::wondering::Wonder;
use std::os::unix::fs::PermissionsExt;

/// An answerer that says one thing and remembers what it was asked.
struct Told(Mutex<Vec<(Wonder, serde_json::Value)>>);

impl Answers for Told {
    fn answer(&self, wonder: Wonder, args: &serde_json::Value) -> Answered {
        self.0
            .lock()
            .expect("recording")
            .push((wonder, args.clone()));
        Answered::Told {
            said: serde_json::json!({ "text": "{\"scores\":[]}", "model": "a-helper" }),
        }
    }
}

/// A tool whose first reply is a question and whose second is a result. Every call it is handed is
/// appended to `calls`, which is where the test reads what the resumption carried: quoting JSON
/// back out of a shell would be testing the fixture rather than the loop.
fn fixture(dir: &std::path::Path, wonder: &serde_json::Value) -> std::path::PathBuf {
    let script = dir.join("supplier");
    let asked = serde_json::json!({ "ok": true, "result": [{ "shown": wonder }] });
    let done = serde_json::json!({ "ok": true, "result": [{ "said": "finished" }] });
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncall=$(/bin/cat)\nprintf '%s\\n' \"$call\" >> calls\n\
             case \"$call\" in\n  *answered*) printf '%s\\n' '{done}' ;;\n  \
             *) printf '%s\\n' '{asked}' ;;\nesac\n"
        ),
    )
    .expect("fixture script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("executable");
    script
}

/// What the tool was handed, one call per line.
fn calls(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

fn ran(dir: &std::path::Path, wonder: &serde_json::Value, knows: Arc<Told>) -> Output {
    let supplied = SuppliedTool {
        card: serde_json::from_value(serde_json::json!({
            "name": "probe", "description": "probe", "parameters": {}
        }))
        .expect("card"),
        program: fixture(dir, wonder).display().to_string(),
        asks: Arc::new(crate::question::Unanswered),
        holds: Arc::new(crate::holding::Screenless),
        knows,
        configured: String::new(),
    };
    supplied.run(
        &serde_json::json!({}),
        &crate::ops::Real::new(dir.to_path_buf()),
        &crate::Uncancelled,
    )
}

#[test]
fn a_tool_that_asks_magi_is_answered_and_run_again() {
    let dir = magi_model::scratch::Scratch::new("magi-supplier", "wonder");
    let knows = Arc::new(Told(Mutex::default()));
    let out = ran(
        &dir,
        &serde_json::json!({
            "shown": "wonder", "wonder": "helper", "about": "scoring",
            "args": { "role": "search", "instruction": "rank these" }
        }),
        Arc::clone(&knows),
    );

    let asked = knows.0.lock().expect("recording");
    assert_eq!(asked.len(), 1, "the question never reached magi");
    assert_eq!(asked[0].0, Wonder::Helper);
    assert_eq!(asked[0].1["role"], "search");

    let calls = calls(&dir);
    assert_eq!(calls.len(), 2, "the call was not run again: {calls:?}");
    assert!(!calls[0].contains("answered"), "{}", calls[0]);
    assert!(
        calls[1].contains("scores") && calls[1].contains("a-helper"),
        "the model's answer did not reach the second half of the call: {}",
        calls[1]
    );
    assert_eq!(out.content, "finished");
}

#[test]
fn a_question_magi_does_not_know_is_refused_rather_than_guessed_at() {
    let dir = magi_model::scratch::Scratch::new("magi-supplier", "wonder-unknown");
    let knows = Arc::new(Told(Mutex::default()));
    let out = ran(
        &dir,
        &serde_json::json!({ "shown": "wonder", "wonder": "the-weather" }),
        Arc::clone(&knows),
    );

    assert!(
        knows.0.lock().expect("recording").is_empty(),
        "a verb magi has no answer for was put to it anyway"
    );
    let calls = calls(&dir);
    assert_eq!(calls.len(), 2, "the refusal did not resume the call");
    assert!(
        calls[1].contains("refused") && calls[1].contains("the-weather"),
        "{}",
        calls[1]
    );
    assert_eq!(out.content, "finished");
}
