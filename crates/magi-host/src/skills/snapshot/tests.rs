use super::*;
use magi_proto::{MessageId, SessionId, workflow::SourceClass};

pub(crate) fn entry() -> Entry {
    Entry::Workflow {
        id: MessageId::new("u1"),
        text: "/skill review ; $(touch nope)".into(),
        aside: "existing context".into(),
        images: Vec::new(),
        workflow: Box::new(Workflow {
            name: "review".into(),
            description: "Révision 日本語".into(),
            source: SourceClass::Project,
            root: "/work/.magi/skills".into(),
            digest: "a".repeat(64),
            body: "Inspect changes.\r\nNever run a script.\n".into(),
            arguments: "; $(touch nope)\n\"quoted\"".into(),
        }),
    }
}

#[test]
fn workflow_snapshot_round_trips_without_optional_user_fields() {
    let selected = entry();
    let record = Record::Entry {
        cursor: Cursor(9),
        entry: selected.clone(),
    };
    let wire = serde_json::to_value(&record).expect("record");
    assert_eq!(wire["entry"]["type"], "workflow");
    assert_eq!(
        serde_json::from_value::<Record>(wire).expect("restored"),
        record
    );
    assert!(check(&selected, Cursor(9)).is_ok());
    for encoding in [magi_ipc::Wire::Json, magi_ipc::Wire::Cbor] {
        let bytes = encoding.write(&record).expect("record encoding");
        assert_eq!(
            magi_ipc::Wire::read::<Record>(&bytes).expect("record decoding"),
            record
        );
    }
    let mut changed = serde_json::to_value(&selected).expect("entry");
    changed["workflow"]["execute"] = true.into();
    assert!(serde_json::from_value::<Entry>(changed).is_err());
}

#[cfg(test)]
#[test]
fn workflow_snapshot_context_preserves_body_and_quotes_arguments_as_data() {
    let selected = entry();
    let Entry::Workflow { workflow, .. } = &selected else {
        panic!("workflow");
    };
    let rendered = context(workflow);
    assert!(rendered.contains(&workflow.body));
    assert!(rendered.ends_with(&serde_json::Value::String(workflow.arguments.clone()).to_string()));
    let restored = crate::context::of_entries(std::slice::from_ref(&selected));
    let magi_model::Content::Text { text, .. } = &restored.messages[0].content[0] else {
        panic!("text");
    };
    assert!(text.contains("existing context"));
    assert!(text.contains(&rendered));
    assert_eq!(crate::context::rewind_point(&[selected]), Some(0));
}

#[test]
fn workflow_snapshot_rejects_invalid_provenance_and_bounds() {
    let Entry::Workflow { workflow, .. } = entry() else {
        panic!("workflow");
    };
    let workflow = *workflow;
    for changed in [
        Workflow {
            name: "../escape".into(),
            ..workflow.clone()
        },
        Workflow {
            root: "/work/../other".into(),
            ..workflow.clone()
        },
        Workflow {
            digest: "A".repeat(64),
            ..workflow.clone()
        },
        Workflow {
            body: "x".repeat(FILE_MOST + 1),
            ..workflow.clone()
        },
        Workflow {
            body: "\0".into(),
            ..workflow.clone()
        },
        Workflow {
            arguments: "x".repeat(4 * 1024 * 1024 + 1),
            ..workflow.clone()
        },
        Workflow {
            description: "line\nbreak".into(),
            ..workflow.clone()
        },
    ] {
        assert!(validate(&changed).is_err());
    }
    assert!(
        validate(&Workflow {
            body: "x".repeat(FILE_MOST),
            ..workflow
        })
        .is_ok()
    );
}

#[cfg(test)]
#[test]
fn workflow_snapshot_replay_expansion_is_refused_before_admission() {
    let mut selected = entry();
    if let Entry::Workflow { text, workflow, .. } = &mut selected {
        *text = "\u{1}".repeat(1_300_000);
        workflow.body = "\u{1}".repeat(61_440);
    }
    let record = Record::Entry {
        cursor: Cursor(1),
        entry: selected.clone(),
    };
    assert!(serde_json::to_vec(&record).expect("record").len() < magi_ipc::family::MAX_FRAME_BYTES);
    assert!(
        check(&selected, Cursor(1))
            .expect_err("too large")
            .contains("replay")
    );
    let mut session = crate::session::Session::recorded(SessionId::new("workflow"), Vec::new());
    assert!(
        session
            .admit(crate::session::admission::Request::Opening(selected))
            .is_err()
    );
    assert!(session.idle());
    assert!(session.entries().is_empty());
}

#[cfg(test)]
#[test]
fn workflow_snapshot_events_restore_selection_and_count_its_bytes() {
    let selected = entry();
    let mut session = crate::session::Session::recorded(SessionId::new("workflow"), Vec::new());
    let mut events = session.subscribe();
    let cursor = session.commit(selected.clone()).expect("commit");
    let magi_proto::HarnessEvent::WorkflowMessage { workflow, text, .. } =
        events.try_recv().expect("event")
    else {
        panic!("workflow event");
    };
    let Entry::Workflow {
        workflow: expected,
        text: expected_text,
        ..
    } = &selected
    else {
        panic!("workflow entry");
    };
    assert_eq!(&workflow, expected);
    assert_eq!(&text, expected_text);
    let replayed = crate::session::Session::restored(
        SessionId::new("restored"),
        vec![(cursor, selected.clone())],
    )
    .expect("restore");
    assert_eq!(replayed.entries(), &[selected]);
    assert!(weight(&workflow) > workflow.body.len() + workflow.arguments.len());
}

#[test]
fn workflow_snapshot_prompt_byte_limit_counts_provenance_and_arguments() {
    let mut selected = entry();
    if let Entry::Workflow {
        text,
        aside,
        workflow,
        ..
    } = &mut selected
    {
        *text = "x".repeat(PROMPT_MOST - aside.len() - weight(workflow));
    }
    assert!(check(&selected, Cursor(1)).is_ok());
    if let Entry::Workflow { text, .. } = &mut selected {
        text.push('x');
    }
    assert!(
        check(&selected, Cursor(1))
            .expect_err("oversized prompt")
            .contains("admission byte limit")
    );
}
