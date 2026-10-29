use magi_proto::{
    Entry, MessageId,
    workflow::{SourceClass, Summary, Workflow},
};
use magi_tui::{
    complete::{self, Kind},
    transcript::{self, Detail},
};

#[test]
fn completion_emits_only_explicit_workflow_invocations_and_ignores_invalid_metadata() {
    let mut entries = vec![Summary {
        name: "review".into(),
        description: "Révision 日本語".into(),
        source: SourceClass::Project,
        error: None,
    }];
    entries.push(Summary {
        name: "refused".into(),
        description: String::new(),
        source: SourceClass::Agents,
        error: Some("invalid metadata".into()),
    });
    let resolve = |line: &str| {
        complete::resolve_workflows(
            line,
            line.chars().count(),
            &|_| Vec::new(),
            &|_| Vec::new(),
            &entries,
        )
    };
    let popup = resolve("/skill re").expect("completion");
    assert_eq!(popup.kind, Kind::Skill);
    assert_eq!(popup.token_start, 0);
    assert_eq!(popup.candidates.len(), 1);
    assert_eq!(popup.current().expect("candidate").value, "/skill review");
    for text in [
        "/review",
        " /skill re",
        "ordinary /skill re",
        "/skill review argument",
        "/skill review ",
    ] {
        assert!(resolve(text).is_none(), "{text}");
    }
    for width in [20, 80, 160] {
        let rows = complete::render(&popup, width);
        assert!(rows.iter().all(|row| row.width() <= usize::from(width)));
    }
}

#[test]
fn workflow_transcript_renders_source_and_digest_without_body_expansion() {
    let entry = Entry::Workflow {
        id: MessageId::new("u1"),
        text: "/skill review literal".into(),
        aside: String::new(),
        images: Vec::new(),
        workflow: Box::new(Workflow {
            name: "review".into(),
            description: "Révision 日本語".into(),
            source: SourceClass::Project,
            root: "/work/日本語\u{1b}[31m/.magi/skills".into(),
            digest: "a".repeat(64),
            body: "SECRET_BODY_NOT_FOR_TRANSCRIPT".into(),
            arguments: "literal".into(),
        }),
    };
    for width in [20, 80, 160] {
        let rows = transcript::render(std::slice::from_ref(&entry), width, Detail::Preview);
        assert!(rows.iter().all(|row| row.width() <= usize::from(width)));
        let text = rows
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("WORKFLOW"));
        assert!(text.contains("Project"));
        assert!(text.contains("SHA-256"));
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains("SECRET_BODY_NOT_FOR_TRANSCRIPT"));
        if width >= 80 {
            assert!(text.contains("日本語"));
            assert!(text.contains("\\u{1b}[31m/.magi/skills"));
            assert!(text.contains(&"a".repeat(64)));
        }
    }
}

#[test]
fn menu_rows_clip_and_pad_by_terminal_columns() {
    for width in [0, 1, 2, 8, 20, 80] {
        for value in ["日本語", "e\u{301}cole", "review"] {
            let row = magi_tui::menu::row(
                &magi_tui::menu::Row {
                    value,
                    detail: "Révision 日本語",
                    selected: true,
                    ready: true,
                    value_width: 12,
                },
                "",
                width,
            );
            assert_eq!(row.width(), usize::from(width));
        }
    }
}
