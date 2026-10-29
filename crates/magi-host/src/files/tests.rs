use super::*;
use magi_proto::tooling::FileMutation;

fn subject(path: &str) -> Subject<'_> {
    Subject {
        supplier: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        cwd: "/work",
        path,
    }
}
fn capability(operation: FileOperation) -> FileCapability {
    FileCapability {
        version: 1,
        operation,
    }
}
fn reserve(
    files: &Files,
    path: &str,
    operation: FileOperation,
) -> Result<(FileRevision, Receipt), String> {
    files.call(7).reserve(subject(path), &capability(operation))
}
fn observation(digit: &str) -> FileMetadata {
    FileMetadata {
        absence: None,
        revision_contract: Some(1),
        mutation: None,
        observations: vec![FileObservation {
            path_key: "b".repeat(64),
            revision: format!("cfr1:{}", digit.repeat(64)),
            coverage: "snapshot".into(),
        }],
    }
}
fn read(files: &Files, path: &str, digit: &str) {
    let (_, receipt) = reserve(files, path, FileOperation::Read).expect("native read admitted");
    receipt.delivered(&observation(digit), false);
}
fn expected(revision: &FileRevision) -> String {
    serde_json::to_value(revision).expect("guard")["expected_revision"]
        .as_str()
        .expect("expected")
        .into()
}

#[test]
fn progress_evidence_is_delivered_scoped_and_stable_across_identical_rereads() {
    let files = Files::default();
    let empty = files.progress(subject("source"));
    let (_, pending) = reserve(&files, "source", FileOperation::Read).expect("read");
    assert_eq!(files.progress(subject("source")), empty);
    pending.delivered(&observation("1"), true);
    assert_eq!(files.progress(subject("source")), empty);
    read(&files, "source", "1");
    let delivered = files.progress(subject("source"));
    assert_ne!(delivered, empty);
    read(&files, "source", "1");
    assert_eq!(files.progress(subject("source")), delivered);
    for other in [
        subject("another"),
        Subject {
            cwd: "/other",
            ..subject("source")
        },
        Subject {
            supplier: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ..subject("source")
        },
    ] {
        assert_eq!(files.progress(other), empty);
    }
    assert_eq!(Files::default().progress(subject("source")), empty);
    let _ = reserve(&files, "source", FileOperation::Edit).expect("edit");
    assert_eq!(files.progress(subject("source")), Some((true, None)));
    let huge = "x".repeat(PATH_BYTES + 1);
    assert_eq!(files.progress(subject(&huge)), None);
    files.retire();
    assert_eq!(files.progress(subject("source")), None);
}

#[test]
fn only_delivered_reads_admit_existing_mutations_and_pending_or_abandoned_reads_do_not() {
    let files = Files::default();
    let (_, pending) = reserve(&files, "source", FileOperation::Read).expect("read");
    assert!(reserve(&files, "source", FileOperation::Edit).is_err());
    pending.delivered(&observation("1"), false);
    let (edit, _) = reserve(&files, "source", FileOperation::Edit).expect("delivered edit");
    assert_eq!(expected(&edit), format!("cfr1:{}", "1".repeat(64)));
    assert!(
        reserve(&files, "source", FileOperation::Edit).is_err(),
        "dispatched mutation retained eligibility"
    );
    assert!(
        reserve(&files, "source", FileOperation::Write).is_err(),
        "unresolved mutation allowed a blind new create"
    );
    let (create, _) = reserve(&files, "new-file", FileOperation::Write).expect("no-replace only");
    assert_eq!(expected(&create), "absent");
    assert_ne!(edit.mutation_id(), create.mutation_id());
}

#[test]
fn child_sibling_supplier_cwd_and_unobserved_aliases_never_refresh_another_agent() {
    let parent = Files::default();
    let child = Files::default();
    read(&child, "source", "2");
    assert!(reserve(&parent, "source", FileOperation::Edit).is_err());
    read(&parent, "source", "1");
    for other in [
        Subject {
            supplier: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ..subject("source")
        },
        Subject {
            cwd: "/another-work",
            ..subject("source")
        },
        subject("./source"),
    ] {
        assert!(
            parent
                .call(7)
                .reserve(other, &capability(FileOperation::Edit))
                .is_err()
        );
    }
    let (revision, _) =
        reserve(&parent, "source", FileOperation::Edit).expect("own delivered observation");
    assert_eq!(expected(&revision), format!("cfr1:{}", "1".repeat(64)));
}

#[test]
fn failed_old_and_duplicate_read_completions_cannot_replace_the_latest_delivered_revision() {
    let files = Files::default();
    let (_, old) = reserve(&files, "source", FileOperation::Read).expect("old");
    let (_, newest) = reserve(&files, "source", FileOperation::Read).expect("new");
    newest.delivered(&observation("2"), false);
    old.delivered(&observation("1"), false);
    newest.delivered(&observation("3"), false);
    let (revision, _) = reserve(&files, "source", FileOperation::Edit).expect("latest");
    assert_eq!(expected(&revision), format!("cfr1:{}", "2".repeat(64)));
    let (_, failed) = reserve(&files, "source", FileOperation::Read).expect("read again");
    failed.delivered(&observation("4"), true);
    assert!(reserve(&files, "source", FileOperation::Edit).is_err());
}

#[test]
fn committed_mutations_refresh_their_own_receipt_but_unknown_or_mismatched_outcomes_require_reread()
{
    let files = Files::default();
    read(&files, "source", "1");
    let (revision, receipt) = reserve(&files, "source", FileOperation::Edit).expect("edit");
    let mut metadata = observation("2");
    metadata.mutation = Some(FileMutation {
        id: revision.mutation_id().expect("logical id").into(),
        outcome: "committed".into(),
        durability: Some("unconfirmed".into()),
    });
    receipt.delivered(&metadata, true);
    let (next, pending) =
        reserve(&files, "source", FileOperation::Edit).expect("commit despite renderer failure");
    assert_eq!(expected(&next), format!("cfr1:{}", "2".repeat(64)));
    let unknown = FileMetadata {
        absence: None,
        revision_contract: Some(1),
        observations: Vec::new(),
        mutation: Some(FileMutation {
            id: next.mutation_id().expect("id").into(),
            outcome: "unknown".into(),
            durability: None,
        }),
    };
    pending.delivered(&unknown, true);
    assert!(reserve(&files, "source", FileOperation::Edit).is_err());
    read(&files, "source", "3");
    let (_, bad) = reserve(&files, "source", FileOperation::Edit).expect("edit");
    bad.delivered(&metadata, false);
    assert!(reserve(&files, "source", FileOperation::Edit).is_err());
}

#[test]
fn bounded_eviction_never_waives_overwrite_guards_or_resurrects_an_evicted_pending_read() {
    let files = Files::default();
    let (_, old) = reserve(&files, "old", FileOperation::Read).expect("pending old");
    for index in 0..ENTRIES {
        read(&files, &format!("file-{index}"), "1");
    }
    assert_eq!(files.state.lock().expect("state").entries.len(), ENTRIES);
    old.delivered(&observation("2"), false);
    assert!(reserve(&files, "old", FileOperation::Edit).is_err());
    assert_eq!(
        expected(
            &reserve(&files, "old", FileOperation::Write)
                .expect("create guard")
                .0
        ),
        "absent"
    );
    assert_eq!(files.state.lock().expect("state").entries.len(), ENTRIES);
}

#[cfg(test)]
#[test]
fn resume_and_restoration_discard_receipts_and_retire_every_old_pending_delivery_context() {
    use crate::session::Session;
    use magi_proto::SessionId;
    let mut session = Session::recorded(SessionId::new("same"), Vec::new());
    let previous = session.file_revisions();
    read(&previous, "source", "1");
    let (_, late) = reserve(&previous, "late", FileOperation::Read).expect("pending");
    session.resume_prepared(magi_journal::Journal::recorded(
        SessionId::new("same"),
        Vec::new(),
    ));
    let current = session.file_revisions();
    assert!(!current.same(&previous));
    late.delivered(&observation("2"), false);
    assert!(reserve(&current, "source", FileOperation::Edit).is_err());
    assert!(reserve(&current, "late", FileOperation::Edit).is_err());
    assert!(reserve(&previous, "source", FileOperation::Read).is_err());
    let stored_read = magi_proto::Entry::Tool {
        id: magi_proto::ToolCallId::new("same-read"),
        name: "look".into(),
        args: r#"{"path":"source"}"#.into(),
        thought_signature: None,
        result: Some(magi_proto::ToolResult {
            images: Vec::new(),
            output: "previously observed source".into(),
            is_error: false,
            shown: None,
        }),
    };
    let restored = Session::restored(
        SessionId::new("same"),
        vec![(magi_proto::Cursor(42), stored_read)],
    )
    .expect("sparse durable restoration")
    .file_revisions();
    assert!(reserve(&restored, "source", FileOperation::Edit).is_err());
}

#[test]
fn invalid_bounds_and_exhausted_logical_ids_refuse_without_source_access_or_state_changes() {
    let files = Files::default();
    for path in ["", "a\0b", &"x".repeat(PATH_BYTES + 1)] {
        assert!(reserve(&files, path, FileOperation::Read).is_err());
    }
    assert!(files.state.lock().expect("state").entries.is_empty());
    files.state.lock().expect("state").next = u64::MAX;
    assert!(reserve(&files, "/does-not-exist", FileOperation::Write).is_err());
    assert!(files.state.lock().expect("state").entries.is_empty());
}

#[test]
fn unresolved_mutations_cannot_be_evicted_or_retried_as_new_creations_and_failed_reads_do_not_clear_them()
 {
    let files = Files::default();
    for index in 0..ENTRIES {
        let (revision, receipt) = reserve(&files, &format!("file-{index}"), FileOperation::Write)
            .expect("new guarded creation");
        let unknown = FileMetadata {
            absence: None,
            revision_contract: Some(1),
            observations: Vec::new(),
            mutation: Some(FileMutation {
                id: revision.mutation_id().expect("id").into(),
                outcome: "unknown".into(),
                durability: None,
            }),
        };
        receipt.delivered(&unknown, true);
    }
    assert!(
        reserve(&files, "new-target", FileOperation::Read).is_err(),
        "unresolved entry evicted"
    );
    assert!(
        reserve(&files, "file-0", FileOperation::Write).is_err(),
        "unknown retried as absent creation"
    );
    let (_, failed) =
        reserve(&files, "file-0", FileOperation::Read).expect("recovery read at capacity");
    failed.delivered(&FileMetadata::default(), true);
    assert!(
        reserve(&files, "file-0", FileOperation::Write).is_err(),
        "failed read cleared unknown outcome"
    );
    read(&files, "file-0", "1");
    assert!(
        reserve(&files, "new-target", FileOperation::Read).is_ok(),
        "recovered entry remained pinned"
    );
    assert_eq!(files.state.lock().expect("state").entries.len(), ENTRIES);
    assert!(
        reserve(&files, "file-1", FileOperation::Write).is_err(),
        "another unresolved key was evicted"
    );
}

#[test]
fn only_requested_successful_absence_delivery_resolves_creation_without_an_overwrite_receipt() {
    let files = Files::default();
    let (revision, pending) =
        reserve(&files, "missing", FileOperation::Write).expect("create attempt");
    pending.delivered(
        &FileMetadata {
            mutation: Some(FileMutation {
                id: revision.mutation_id().expect("id").into(),
                outcome: "unknown".into(),
                durability: None,
            }),
            revision_contract: Some(1),
            ..FileMetadata::default()
        },
        true,
    );
    let proof = FileMetadata {
        revision_contract: Some(1),
        absence: Some(magi_proto::tooling::FileAbsence {
            version: 1,
            path_key: "b".repeat(64),
        }),
        ..FileMetadata::default()
    };
    let (read, failed) = reserve(&files, "missing", FileOperation::Read).expect("recovery read");
    assert!(read.recovering());
    failed.delivered(&proof, true);
    assert!(reserve(&files, "missing", FileOperation::Write).is_err());
    let (_, unsupported) = reserve(&files, "missing", FileOperation::Read).expect("recovery");
    let mut invalid = proof.clone();
    invalid.absence.as_mut().expect("proof").version = 2;
    unsupported.delivered(&invalid, false);
    assert!(reserve(&files, "missing", FileOperation::Write).is_err());
    let (_, success) = reserve(&files, "missing", FileOperation::Read).expect("recovery");
    success.delivered(&proof, false);
    assert!(
        reserve(&files, "missing", FileOperation::Edit).is_err(),
        "absence enabled existing edit"
    );
    assert_eq!(
        expected(
            &reserve(&files, "missing", FileOperation::Write)
                .expect("create only")
                .0
        ),
        "absent"
    );
}
