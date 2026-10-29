use super::*;

#[test]
fn workflow_snapshot_projection_and_raw_replay_are_lossless() {
    let entry = crate::skills::snapshot::tests::entry();
    let wire = turn(Cursor(7), &entry, &Beside::default()).expect("projection");
    assert_eq!(wire["kind"], "workflow");
    assert_eq!(wire["role"], "user");
    assert_eq!(
        rebuild(&wire).expect("object record"),
        (Cursor(7), entry.clone())
    );
    let mut string = wire.clone();
    string["raw"] = wire["raw"].to_string().into();
    assert_eq!(
        rebuild(&string).expect("string record"),
        (Cursor(7), entry.clone())
    );
    assert_eq!(super::super::shape::stubbed(&entry, 1), entry);
    string["truncated"] = true.into();
    assert!(rebuild(&string).is_err());
}

#[test]
fn workflow_snapshot_fitting_refuses_instead_of_stubbing_and_does_not_dial() {
    let scribe = Scribe::unconnected("/nonexistent/workflow.sock".into(), &SessionId::new("test"));
    let mut entry = crate::skills::snapshot::tests::entry();
    assert!(scribe.validate_workflow(&entry).is_ok());
    let full = scribe
        .fitted("observe", Cursor(1), &entry, &Beside::default())
        .expect("full");
    assert_eq!(rebuild(&full[1]).expect("restore").1, entry);
    if let Entry::Workflow { text, workflow, .. } = &mut entry {
        *text = "\u{1}".repeat(1_300_000);
        workflow.body = "\u{1}".repeat(61_440);
    }
    let wire = turn(Cursor(1), &entry, &Beside::default()).expect("write");
    assert!(magi_ipc::family::fits("observe", &["test".into(), wire]));
    assert!(matches!(
        scribe.validate_workflow(&entry),
        Err(Fault::Refused(_))
    ));
    if let Entry::Workflow { text, .. } = &mut entry {
        *text = "\u{1}".repeat(1_000_000);
    }
    assert!(scribe.validate_workflow(&entry).is_ok());
    let huge_identity = Scribe::unconnected(
        "/nonexistent/workflow.sock".into(),
        &SessionId::new("s".repeat(9 * 1024 * 1024)),
    );
    assert!(matches!(
        huge_identity.validate_workflow(&entry),
        Err(Fault::Refused(_))
    ));
}
