use super::*;
use magi_proto::{HarnessEvent, workflow::SourceClass};
use magi_tui::overlay::Overlay;

fn summary() -> Summary {
    Summary {
        name: "review".into(),
        description: "Révision 日本語".into(),
        source: SourceClass::Project,
        error: None,
    }
}

#[test]
fn workflow_catalog_is_lazy_once_and_read_only_views_do_not_request_it() {
    let mut app = App::new();
    assert!(app.workflow_request().is_none());
    app.editor.insert_str("/skill re");
    assert_eq!(app.workflow_request(), Some(UiCommand::RefreshWorkflows));
    assert!(app.workflow_request().is_none());
    app.apply(HarnessEvent::WorkflowsListed {
        entries: vec![summary()],
        warnings: Vec::new(),
        error: None,
    });
    let Some(Overlay::Completion(popup)) = &app.overlay else {
        panic!("workflow popup");
    };
    assert_eq!(popup.current().expect("candidate").value, "/skill review");
    assert!(
        popup
            .current()
            .expect("candidate")
            .detail
            .contains("日本語")
    );
    app.clear_workflows();
    assert!(app.overlay.is_none());
    app.view_only = true;
    assert!(app.workflow_request().is_none());
}

#[test]
fn workflow_catalog_failures_clear_stale_choices_without_resetting_input_or_scroll() {
    let mut app = App::new();
    app.editor.insert_str("/skill re");
    app.listed_workflows(vec![summary()], Vec::new(), None);
    app.scrollback.to_top();
    let view = app.scrollback.view(24);
    app.apply(HarnessEvent::WorkflowsListed {
        entries: Vec::new(),
        warnings: Vec::new(),
        error: Some("workflow catalog refused".into()),
    });
    assert_eq!(app.editor.text(), "/skill re");
    assert!(!app.scrollback.is_following());
    assert_eq!(app.scrollback.view(24), view);
    assert!(app.workflows.is_empty());
    assert!(app.overlay.is_none());
    assert!(app.workflow_request().is_none());
    assert_eq!(app.refresh_workflows(), UiCommand::RefreshWorkflows);
    assert!(app.entries.iter().any(|entry| matches!(entry, magi_proto::Entry::Notice { text } if text == "workflow catalog refused")));
}

#[test]
fn workflow_catalog_is_cleared_when_switching_sessions_and_ignores_later_lines() {
    let mut app = App::new();
    app.listed_workflows(vec![summary()], Vec::new(), None);
    app.attach_to(None);
    assert!(app.workflows.is_empty());
    app.editor.insert_str("ordinary\n/skill re");
    assert!(app.workflow_request().is_none());
    app.refresh_completion(&|_| Vec::new());
    assert!(app.overlay.is_none());
}
