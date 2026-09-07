//! The shipped examples, run.
//!
//! pi ships roughly seventy-eight example extensions. That is not documentation — it is how they
//! know the extension surface works, and ours had never been used by anybody who did not write
//! it. An example that does not load is worse than no example, because somebody copies it.
//!
//! These load each file exactly as a plugin directory would and check that it declared what it
//! says it declares. What they cannot check is that `rg` finds anything, which is the tool's
//! business rather than the surface's.

use magi_lua::Engine;

/// One example, read from the tree at run time — the way a plugin directory reads one.
fn example(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugin")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()))
}

#[test]
fn every_example_loads_in_a_plain_vm() {
    // The sandbox is on, nothing is lent, and no configuration has run first. That is what a
    // plugin directory hands a file, so it is what these are held to.
    for name in ["ripgrep.lua", "status-line.lua"] {
        let mut engine = Engine::new();
        engine
            .run(&example(name), name)
            .unwrap_or_else(|why| panic!("{name}: {why}"));
    }
}

#[test]
fn the_tool_example_declares_a_tool_with_everything_a_tool_owes() {
    let mut engine = Engine::new();
    engine
        .run(&example("ripgrep.lua"), "ripgrep.lua")
        .expect("loads");
    engine.harvest();

    let declared = engine.tools();
    let (_, spec) = declared
        .iter()
        .find(|(name, _)| name == "ripgrep")
        .unwrap_or_else(|| panic!("nothing named ripgrep: {declared:?}"));

    assert!(
        spec.get("description").is_some(),
        "the model is told what it does"
    );
    assert!(spec.get("parameters").is_some(), "and held to a schema");
    assert_eq!(
        spec.get("needs").and_then(serde_json::Value::as_str),
        Some("run"),
        "and it says which permission it acts under"
    );
}

#[test]
fn the_watcher_example_registers_a_watcher_and_survives_every_kind_of_event() {
    // The failure this catches is the one that made it necessary: a watcher written when there
    // was one kind of event assumed every event had a `tool` field. Handing it all of them is
    // the whole test.
    let mut engine = Engine::new();
    engine
        .run(&example("status-line.lua"), "status-line.lua")
        .expect("loads");

    for event in [
        serde_json::json!({ "kind": "session.opened", "id": "s", "resumed": false }),
        serde_json::json!({ "kind": "turn.began", "model": "m" }),
        serde_json::json!({ "kind": "turn.ended", "model": "m", "took_ms": 3, "ok": true }),
        serde_json::json!({ "kind": "tool.finished", "tool": "bash", "is_error": false }),
        serde_json::json!({ "kind": "permission.asked", "verb": "run", "about": "git" }),
        serde_json::json!({ "kind": "permission.answered", "verb": "run", "about": "git", "allowed": false }),
        serde_json::json!({ "kind": "context.compacted", "dropped": 2, "kept": 8 }),
        serde_json::json!({ "kind": "provider.retried", "mind": "melchior", "attempt": 1, "of": 3, "delay_ms": 500 }),
    ] {
        engine.call_watchers(&event);
    }

    // Nothing is asserted about the file: `magi.fs.write` refuses when no ops are lent, which is
    // exactly this situation, and the example is written to carry on when it does.
    engine.harvest();
}
