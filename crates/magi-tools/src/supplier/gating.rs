//! What a card asks magi to decide before it runs.

use super::*;
use magi_proto::permit::Action;

fn card(needs: Option<&str>) -> Card {
    Card {
        name: "bash".to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
        needs: needs.map(ToOwned::to_owned),
        deferred: false,
    }
}

#[test]
fn a_tool_that_needs_nothing_is_not_gated() {
    // Asking about something nobody would want a say over turns a prompt into a nuisance, and a
    // nuisance is answered without being read.
    assert!(wants(&card(None), &serde_json::json!({})).is_none());
}

#[test]
fn a_verb_becomes_the_action_magi_already_knows_how_to_ask_about() {
    let read = wants(&card(Some("read")), &serde_json::json!({"path": "/tmp/x"}));
    assert_eq!(
        read,
        Some(Action::Read {
            path: "/tmp/x".to_owned()
        })
    );
    let run = wants(
        &card(Some("run")),
        &serde_json::json!({"command": "rm -rf build"}),
    );
    assert_eq!(
        run,
        Some(Action::Run {
            command: "rm -rf build".to_owned(),
            program: "rm".to_owned(),
        })
    );
}

#[test]
fn a_verb_nobody_recognises_is_guarded_rather_than_waved_through() {
    // A newer casper inventing a verb must not be treated as harmless.
    let odd = wants(&card(Some("teleport")), &serde_json::json!({}));
    assert!(matches!(odd, Some(Action::Run { .. })), "{odd:?}");
}

#[test]
fn a_tool_with_nothing_to_name_is_asked_about_by_its_own_name() {
    // Never as a path: `<cwd>/bash` would be an edit inside the project to `edits` mode.
    for needs in ["read", "write"] {
        let bare = wants(&card(Some(needs)), &serde_json::json!({}));
        assert_eq!(
            bare,
            Some(Action::Run {
                command: "bash".to_owned(),
                program: "bash".to_owned(),
            }),
            "{needs}"
        );
    }
}

#[test]
fn an_mcp_tool_is_asked_about_by_name_whatever_its_path_says() {
    let mut card = card(Some("write"));
    card.name = "mcp__github__create_file".to_owned();
    let asked = wants(&card, &serde_json::json!({ "path": "README.md" }));
    assert_eq!(
        asked,
        Some(Action::Run {
            command: "mcp__github__create_file".to_owned(),
            program: "mcp__github__create_file".to_owned(),
        })
    );
}

#[test]
fn a_reach_that_names_no_host_is_refused_before_anything_runs() {
    let tool = SuppliedTool {
        environ: Default::default(),
        card: card(Some("reach")),
        program: "magi-no-such-casper-anywhere".to_owned(),
        asks: Arc::new(crate::question::Unanswered),
        holds: Arc::new(crate::holding::Screenless),
        knows: Arc::new(crate::holding::Incurious),
        configured: String::new(),
    };
    let out = tool.run(
        &serde_json::json!({ "url": "" }),
        &crate::ops::Real::gated(
            std::env::temp_dir(),
            crate::permit::Ledger::new(),
            Arc::new(crate::approve::AllowAll),
        ),
        &Uncancelled,
    );
    assert!(
        out.is_error && out.content.contains("names no host"),
        "{out:?}"
    );
}

#[test]
fn web_is_asked_about_by_the_host_its_url_names() {
    let reach = |args: serde_json::Value| wants(&card(Some("reach")), &args);
    for (url, host) in [
        ("https://Example.COM/a?b", "example.com"),
        ("http://user:pw@host.test:8080/x", "host.test"),
        ("https://[::1]:443/", "::1"),
        ("example.org/path", "example.org"),
    ] {
        assert_eq!(
            reach(serde_json::json!({ "url": url })),
            Some(Action::Network {
                host: host.to_owned()
            }),
            "{url}"
        );
    }
    assert_eq!(
        reach(serde_json::json!({})),
        Some(Action::Network {
            host: String::new()
        }),
        "named nowhere, and refused before anybody is asked"
    );
}
