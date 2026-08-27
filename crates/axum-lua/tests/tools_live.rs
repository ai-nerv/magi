//! The shipped Lua tools, against siblings that are actually running.
//!
//! Skips quietly when nothing is listening: a test that needs someone else's daemon must not
//! fail a build on a machine that has none. When one *is* running, this is the only thing that
//! proves the stub, the socket primitive and the tool declaration line up — every one of those
//! works when axum talks to itself.

use axum_lua::Engine;
use axum_tools::Registry;
use std::cell::RefCell;
use std::rc::Rc;

const HEXE_TOOL: &str = include_str!("../../../config/tools/hexe.lua");
const HEXE_STUB: &str = include_str!("../../../config/stubs/hexe.lua");

fn is_live(name: &str) -> bool {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::read_dir(runtime.join(name)).is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("api@"))
    })
}

#[test]
fn the_hexe_tool_answers_when_a_mux_is_running() {
    let mut engine = Engine::new();
    engine.install_stubs(&[("hexe".to_owned(), HEXE_STUB.to_owned())]);
    engine
        .run(HEXE_TOOL, "hexe.lua")
        .expect("the tool declaration must run");

    let engine = Rc::new(RefCell::new(engine));
    let mut registry = Registry::new();
    axum_lua::tool::install(Rc::clone(&engine), &mut registry);

    assert!(registry.get("hexe").is_some(), "the tool registered");

    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    let output = registry.call("hexe", &serde_json::json!({ "what": "verbs" }), &ops);

    if !is_live("hexe") {
        // No mux: the tool must say so plainly rather than fail. A tool that errors when the
        // thing it asks about is simply absent teaches the model to stop asking.
        assert!(!output.is_error, "{}", output.content);
        assert!(
            output.content.contains("no hexe session"),
            "{}",
            output.content
        );
        return;
    }

    assert!(
        !output.is_error,
        "hexe is running but the tool failed: {}",
        output.content
    );
    assert!(
        output.content.contains("panes") || output.content.contains("verbs"),
        "the mux answered with something unexpected: {}",
        output.content
    );
    eprintln!(
        "hexe tool answered: {}",
        &output.content[..output.content.len().min(120)]
    );
}

#[test]
fn a_lua_tool_reports_an_absent_sibling_as_information_not_a_failure() {
    let mut engine = Engine::new();
    // No stubs installed at all, which is what an install without `make configs` looks like.
    engine.run(HEXE_TOOL, "hexe.lua").expect("run");
    let engine = Rc::new(RefCell::new(engine));
    let mut registry = Registry::new();
    axum_lua::tool::install(Rc::clone(&engine), &mut registry);

    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    let output = registry.call("hexe", &serde_json::json!({}), &ops);
    assert!(output.is_error, "a missing stub is a real problem");
    assert!(
        output.content.contains("make configs"),
        "{}",
        output.content
    );
}
