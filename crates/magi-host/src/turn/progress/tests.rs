use super::*;

#[test]
fn canonical_objects_preserve_arrays_strings_tool_identity_and_repairs() {
    let key = fingerprint("read", r#"{"b":{"z":2,"a":1},"a":[" x ",3]}"#);
    assert_eq!(
        key,
        fingerprint("read", r#"{ "a":[" x ",3],"b":{"a":1,"z":2}}"#)
    );
    for (name, args) in [
        ("edit", r#"{"b":{"z":2,"a":1},"a":[" x ",3]}"#),
        ("read", r#"{"b":{"z":2,"a":1},"a":["x",3]}"#),
        ("read", r#"{"b":{"z":2,"a":1},"a":[3," x "]}"#),
    ] {
        assert_ne!(key, fingerprint(name, args));
    }
    assert_eq!(
        fingerprint("x", "{\"s\":\"a\nb\"}"),
        fingerprint("x", r#"{"s":"a\nb"}"#)
    );
    assert_ne!(fingerprint("x", "broken"), fingerprint("x", "broken "));
    assert_eq!(fingerprint("x", ""), fingerprint("x", "{}"));
}

#[test]
fn completed_failures_suppress_third_and_stop_fourth_without_synthetic_progress() {
    let mut guard = Progress::default();
    let key = fingerprint("x", "{}").expect("key");
    for _ in 0..2 {
        assert_eq!(guard.admit(Some(key)), Ok(()));
        guard.completed(key, &Output::error("failed"));
    }
    assert_eq!(guard.admit(Some(key)), Err(REPAIR));
    assert_eq!(guard.admit(Some(key)), Err(REPEATED));
    assert_eq!(guard.stopped(), Some(REPEATED));
}

#[test]
fn changed_results_and_success_reset_only_their_key_empty_output_does_not() {
    let mut guard = Progress::default();
    let a = fingerprint("x", "{}").expect("key");
    let b = fingerprint("y", "{}").expect("key");
    guard.completed(a, &Output::error("first"));
    guard.completed(b, &Output::error("other"));
    guard.completed(a, &Output::error("second"));
    assert_eq!(guard.failures[&a].streak, 1);
    guard.completed(a, &Output::error(""));
    assert_eq!(guard.failures[&a].streak, 2);
    guard.completed(a, &Output::ok(""));
    assert!(!guard.failures.contains_key(&a));
    assert_eq!(guard.failures[&b].streak, 1);
    assert!(Progress::default().failures.is_empty());
}

#[test]
fn raw_middle_and_native_revisions_count_but_attempt_ids_do_not() {
    let mut a = Output::error("a".repeat(100_000));
    let mut b = a.clone();
    b.content.replace_range(50_000..50_001, "b");
    assert_ne!(outcome(&a), outcome(&b));
    a.files = Some(Box::new(magi_proto::tooling::FileMetadata {
        observations: vec![magi_proto::tooling::FileObservation {
            path_key: "path".into(),
            revision: "one".into(),
            coverage: "full".into(),
        }],
        mutation: Some(magi_proto::tooling::FileMutation {
            id: "first".into(),
            outcome: "refused".into(),
            durability: None,
        }),
        ..Default::default()
    }));
    b = a.clone();
    b.files
        .as_mut()
        .expect("files")
        .mutation
        .as_mut()
        .expect("mutation")
        .id = "second".into();
    assert_eq!(outcome(&a), outcome(&b));
    b.files.as_mut().expect("files").observations[0].revision = "two".into();
    assert_ne!(outcome(&a), outcome(&b));
}

#[test]
fn record_and_argument_limits_fail_closed_without_evicting_failed_keys() {
    let mut guard = Progress::default();
    for n in 0..=MAX_RECORDS {
        let key = fingerprint("x", &format!("{{\"n\":{n}}}")).expect("key");
        guard.completed(key, &Output::error("failed"));
    }
    assert_eq!(guard.failures.len(), MAX_RECORDS);
    assert_eq!(guard.stopped(), Some(CAPACITY));
    let large = format!("{{\"text\":\"{}\"}}", "x".repeat(2 * 1024 * 1024));
    assert!(fingerprint("write", &large).is_some());
    let too_large = "x".repeat(MAX_ARGUMENT_BYTES + 1);
    let mut guard = Progress::default();
    assert_eq!(
        guard.admit(fingerprint("write", &too_large)),
        Err(ARGUMENTS)
    );
    assert_eq!(guard.failures.len(), 0);
}
