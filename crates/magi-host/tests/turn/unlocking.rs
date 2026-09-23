//! A deferred tool unlocked by a lookup is callable from the very next round of the same prompt.

use super::{backend, session};
use magi_testkit::{
    Mind,
    mind::{call_lines, stop_line, text_line},
};
use std::sync::Arc;
use tokio::sync::Mutex;

/// The manual: looking a page up unlocks the tool it describes.
struct Manual;

impl magi_tools::Tool for Manual {
    fn name(&self) -> &str {
        "tools"
    }
    fn description(&self) -> &str {
        "The manual for tools you are not shown"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn run(
        &self,
        _: &serde_json::Value,
        _: &dyn magi_tools::Ops,
        _: &dyn magi_tools::Cancel,
    ) -> magi_tools::Output {
        let mut output = magi_tools::Output::ok("dino() -- the dinosaur game");
        output.unlocks = vec!["dino".to_owned()];
        output
    }
}

/// A tool nobody is shown until the manual has named it.
struct Dino;

impl magi_tools::Tool for Dino {
    fn name(&self) -> &str {
        "dino"
    }
    fn description(&self) -> &str {
        "The dinosaur game"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn deferred(&self) -> bool {
        true
    }
    fn run(
        &self,
        _: &serde_json::Value,
        _: &dyn magi_tools::Ops,
        _: &dyn magi_tools::Cancel,
    ) -> magi_tools::Output {
        magi_tools::Output::ok("played")
    }
}

/// Whether the ask the mind heard declared a tool of this name.
fn declared(ask: &str, name: &str) -> bool {
    let body: serde_json::Value = serde_json::from_str(ask).expect("an ask");
    body["context"]["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == name))
}

#[tokio::test]
async fn a_tool_looked_up_is_there_to_call_in_the_next_round() {
    // The tool list was taken once per prompt, so an unlock only ever counted from the person's
    // next message. A model that looked `dino` up had nothing to call, and ran it in the shell.
    let (session, _dir) = session("unlock");
    let session = Arc::new(session);
    let look = call_lines("look", "tools", r#"{"path":"games/dino"}"#);
    let play = call_lines("play", "dino", "{}");
    let mind = Mind::turns(
        "unlock",
        &[
            &[&look[0], &look[1], &look[2]],
            &[&play[0], &play[1], &play[2]],
            &[&text_line("have fun"), &stop_line()],
        ],
    );
    let mut registry = magi_tools::Registry::new();
    registry.register(Box::new(Manual));
    registry.register(Box::new(Dino));
    magi_host::turn::run(
        &session,
        &backend(&mind),
        &registry,
        &magi_tools::ops::Real::new(std::env::temp_dir()),
        &Arc::new(Mutex::new(None)),
    )
    .await
    .expect("the turn returns");

    let asks = mind.asks();
    assert_eq!(asks.len(), 3, "three rounds");
    assert!(!declared(&asks[0], "dino"), "hidden before it is looked up");
    assert!(
        declared(&asks[1], "dino"),
        "and there in the round right after"
    );
    let held = session.lock().await;
    assert!(
        held.entries().iter().any(|entry| matches!(entry,
            magi_proto::Entry::Tool { name, result: Some(result), .. }
                if name == "dino" && result.output == "played" && !result.is_error)),
        "it was called as a tool and ran"
    );
}
