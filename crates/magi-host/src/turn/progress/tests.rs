use super::*;

fn complete(guard: &mut Progress, key: Key, output: &Output) {
    guard.completed(
        Some(Completion {
            key,
            evidence: outcome(output),
        }),
        output,
    );
}

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
        complete(&mut guard, key, &Output::error("failed"));
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
    complete(&mut guard, a, &Output::error("first"));
    complete(&mut guard, b, &Output::error("other"));
    complete(&mut guard, a, &Output::error("second"));
    assert_eq!(guard.failures[&a].streak, 1);
    complete(&mut guard, a, &Output::error(""));
    assert_eq!(guard.failures[&a].streak, 2);
    complete(&mut guard, a, &Output::ok(""));
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
        complete(&mut guard, key, &Output::error("failed"));
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

#[test]
fn final_image_failure_is_counted_once_and_refusals_do_not_reset_the_guard() {
    use std::{cell::Cell, rc::Rc};
    struct Pixels(Rc<Cell<usize>>);
    impl magi_tools::Tool for Pixels {
        fn name(&self) -> &str {
            "pixels"
        }
        fn description(&self) -> &str {
            "image result"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        fn run(&self, _: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
            self.0.set(self.0.get() + 1);
            Output::ok("source pixels")
        }
    }
    let ran = Rc::new(Cell::new(0));
    let mut registry = Registry::new();
    registry.register(Box::new(Pixels(ran.clone())));
    let calls: Vec<_> = (0..4)
        .map(|index| PendingCall {
            id: index.to_string(),
            name: "pixels".into(),
            arguments: "{}".into(),
        })
        .collect();
    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    let files = crate::files::Files::default();
    let mut guard = Progress::default();
    let scheduled = guard.prepare(&calls, &registry, &ops, &magi_tools::Uncancelled);
    for (index, (call, scheduled)) in calls.iter().zip(scheduled).enumerate() {
        let (mut output, completion) = guard.finish(
            scheduled,
            call,
            &registry,
            &ops,
            &magi_tools::Uncancelled,
            &files,
        );
        if index < 2 {
            assert!(!output.is_error);
            output.is_error = true;
            output.content = format!("spill-{index}: Image attachments omitted");
            assert!(completion.is_some());
        } else {
            assert!(output.is_error && output.content.starts_with("Repeated-call guard:"));
            assert!(completion.is_none());
        }
        guard.completed(completion, &output);
    }
    assert_eq!(ran.get(), 2);
    assert_eq!(
        guard.failures[&fingerprint("pixels", "{}").expect("key")].streak,
        3
    );
    assert_eq!(guard.stopped(), Some(REPEATED));
}

#[test]
fn changed_source_evidence_survives_identical_presented_failures() {
    let mut guard = Progress::default();
    let key = fingerprint("pixels", "{}").expect("key");
    for source in ["first image", "second image"] {
        let evidence = outcome(&Output::ok(source));
        guard.completed(
            Some(Completion { key, evidence }),
            &Output::error("Image attachments omitted"),
        );
        assert_eq!(guard.failures[&key].streak, 1);
    }
    complete(&mut guard, key, &Output::ok("success"));
    assert!(!guard.failures.contains_key(&key));
}
