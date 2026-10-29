#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use serde_json::json;

#[test]
fn prompt_preserves_text_and_literal_resource_metadata_without_fetching() {
    let value = json!({"sessionId":"session", "prompt":[
        {"type":"text", "text":"one"}, {"type":"text", "text":"two"},
        {"type":"resource_link", "uri":"file:///does/not/exist", "name":"$(touch /must-not-create)", "description":"quoted \" resource"},
        {"type":"resource_link", "uri":"https://invalid.example/resource", "name":"network"}
    ]});
    let parsed = parse(value).expect("baseline prompt");
    assert_eq!(parsed.session, "session");
    let UiCommand::SubmitPrompt {
        text,
        aside,
        images,
    } = parsed.command
    else {
        panic!("native prompt")
    };
    assert_eq!(text, "one\ntwo");
    assert!(aside.contains("file:///does/not/exist"));
    assert!(aside.contains("https://invalid.example/resource"));
    assert!(aside.contains("$(touch /must-not-create)"));
    assert!(images.is_empty());
}

#[test]
fn prompt_rejects_unsupported_effects_and_content_before_native_submission() {
    for block in [
        json!({"type":"text", "text":"x", "execute":true}),
        json!({"type":"text", "text":3}),
        json!({"type":"resource_link", "uri":"file:///a", "name":"a", "fetch":true}),
        json!({"type":"resource_link", "uri":"file:///a\n", "name":"a"}),
        json!({"type":"resource_link", "uri":"file:///a"}),
        json!({"type":"image", "mimeType":"image/png", "data":"AAAA"}),
        json!({"type":"resource", "resource":{"uri":"file:///a", "text":"x"}}),
    ] {
        assert!(parse(json!({"sessionId":"s", "prompt":[block]})).is_err());
    }
    assert!(
        parse(json!({"sessionId":"s", "prompt":[{"type":"text", "text":"x"}], "budget":1}))
            .is_err()
    );
}

#[test]
fn prompt_bounds_session_content_count_and_bytes() {
    let text = json!({"type":"text", "text":"x"});
    for blocks in [
        vec![],
        vec![text; 65],
        vec![json!({"type":"text", "text":"x".repeat(BYTES)})],
    ] {
        assert!(parse(json!({"sessionId":"s", "prompt":blocks})).is_err());
    }
    for id in [String::new(), "s".repeat(crate::acp::history::ID_BYTES + 1)] {
        assert!(parse(json!({"sessionId":id, "prompt":[{"type":"text", "text":"x"}]})).is_err());
    }
}
