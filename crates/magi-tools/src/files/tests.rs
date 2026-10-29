use super::*;
use magi_proto::tooling::{FileMutation, FileObservation};
use std::sync::atomic::{AtomicUsize, Ordering};

fn observed() -> FileMetadata {
    FileMetadata {
        absence: None,
        revision_contract: Some(1),
        mutation: None,
        observations: vec![FileObservation {
            path_key: "a".repeat(64),
            revision: format!("cfr1:{}", "b".repeat(64)),
            coverage: "snapshot".into(),
        }],
    }
}

struct Ack(AtomicUsize);
impl Acknowledge for Ack {
    fn delivered(&self, _: &FileMetadata, _: bool) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn native_delivery_is_opaque_session_and_cursor_bound_and_acknowledged_only_once() {
    let owner = Owner::default();
    let ack = Arc::new(Ack(AtomicUsize::new(0)));
    let revision = FileRevision::native(
        &FileCapability {
            version: 1,
            operation: FileOperation::Read,
        },
        None,
    )
    .expect("read");
    let output = Grant::new(revision, FileOperation::Read, owner.clone(), 9, ack.clone()).complete(
        crate::Output {
            files: Some(Box::new(observed())),
            ..crate::Output::ok("bytes")
        },
    );
    let value = serde_json::to_value(&output).expect("ordinary output encoding");
    assert!(value.get("file_delivery").is_none() && value.get("files").is_none());
    let delivery = output.file_delivery.expect("private completion");
    delivery.clone().deliver(&Owner::default(), 9, false);
    delivery.clone().deliver(&owner, 10, false);
    assert_eq!(ack.0.load(Ordering::SeqCst), 0);
    delivery.clone().deliver(&owner, 9, false);
    delivery.deliver(&owner, 9, false);
    assert_eq!(ack.0.load(Ordering::SeqCst), 1);
}

#[test]
fn native_metadata_rejects_unsupported_receipts_and_mismatched_or_incomplete_mutation_outcomes() {
    let mut metadata = observed();
    assert!(valid(&metadata, FileOperation::Read, None, false));
    assert!(!valid(&metadata, FileOperation::Read, None, true));
    for field in ["revision", "coverage", "path_key"] {
        let mut invalid = serde_json::to_value(&metadata).expect("metadata");
        invalid["observations"][0][field] = serde_json::json!("unknown");
        assert!(!valid(
            &serde_json::from_value(invalid).expect("typed fields"),
            FileOperation::Read,
            None,
            false
        ));
    }
    metadata.mutation = Some(FileMutation {
        id: "logical".into(),
        outcome: "committed".into(),
        durability: Some("synced".into()),
    });
    assert!(valid(
        &metadata,
        FileOperation::Write,
        Some("logical"),
        false
    ));
    assert!(valid(
        &metadata,
        FileOperation::Write,
        Some("logical"),
        true
    ));
    assert!(!valid(
        &metadata,
        FileOperation::Write,
        Some("wrong"),
        false
    ));
    metadata.observations.clear();
    assert!(valid(
        &metadata,
        FileOperation::Write,
        Some("logical"),
        false
    ));
    metadata.mutation.as_mut().expect("mutation").durability = None;
    assert!(!valid(
        &metadata,
        FileOperation::Write,
        Some("logical"),
        true
    ));
    metadata.mutation.as_mut().expect("mutation").outcome = "unknown".into();
    assert!(valid(
        &metadata,
        FileOperation::Write,
        Some("logical"),
        true
    ));
    assert!(!valid(
        &metadata,
        FileOperation::Write,
        Some("logical"),
        false
    ));
    metadata.observations = observed().observations;
    assert!(!valid(
        &metadata,
        FileOperation::Write,
        Some("logical"),
        true
    ));
}

#[test]
fn lost_native_outcomes_retain_logical_correlation_without_claiming_refusal_or_replay() {
    let owner = Owner::default();
    let revision = FileRevision::native(
        &FileCapability {
            version: 1,
            operation: FileOperation::Write,
        },
        Some(("absent", "logical-7")),
    )
    .expect("guard");
    let output = Grant::new(
        revision,
        FileOperation::Write,
        owner,
        3,
        Arc::new(Ack(AtomicUsize::new(0))),
    )
    .failed("response lost".into());
    assert!(output.is_error && output.content.contains("Not retried"));
    let metadata = output.files.expect("machine outcome");
    assert_eq!(
        metadata.mutation.as_ref().expect("correlation").id,
        "logical-7"
    );
    assert_eq!(
        metadata.mutation.as_ref().expect("outcome").outcome,
        "unknown"
    );
    assert!(metadata.observations.is_empty());
}

#[test]
fn postcommit_presentation_errors_keep_confirmed_identity_before_any_clipping() {
    let revision = FileRevision::native(
        &FileCapability {
            version: 1,
            operation: FileOperation::Write,
        },
        Some(("absent", "logical-8")),
    )
    .expect("guard");
    let mut metadata = observed();
    metadata.mutation = Some(FileMutation {
        id: "logical-8".into(),
        outcome: "committed".into(),
        durability: Some("unconfirmed".into()),
    });
    let output = Grant::new(
        revision,
        FileOperation::Write,
        Owner::default(),
        7,
        Arc::new(Ack(AtomicUsize::new(0))),
    )
    .complete(crate::Output {
        files: Some(Box::new(metadata.clone())),
        ..crate::Output::ok("rendered")
    });
    let failed = presentation(crate::Output {
        is_error: true,
        ..output
    });
    assert!(
        failed
            .content
            .starts_with("Native file mutation committed; error does not imply rollback.")
    );
    assert_eq!(failed.files.as_deref(), Some(&metadata));
    assert!(failed.file_delivery.is_some());
    let forged = presentation(crate::Output {
        files: Some(Box::new(metadata)),
        ..crate::Output::error("ordinary callback")
    });
    assert_eq!(forged.content, "ordinary callback");
}

#[test]
fn absence_proofs_need_explicit_read_recovery_and_never_authorize_mutations_or_failed_delivery() {
    let cap = FileCapability {
        version: 1,
        operation: FileOperation::Read,
    };
    let metadata = FileMetadata {
        revision_contract: Some(1),
        absence: Some(magi_proto::tooling::FileAbsence {
            version: 1,
            path_key: "a".repeat(64),
        }),
        ..FileMetadata::default()
    };
    assert!(valid(&metadata, FileOperation::Read, None, false));
    assert!(!valid(&metadata, FileOperation::Read, None, true));
    assert!(!valid(&metadata, FileOperation::Write, Some("id"), false));
    let output = || crate::Output {
        files: Some(Box::new(metadata.clone())),
        ..crate::Output::ok("absent")
    };
    let ack = Arc::new(Ack(AtomicUsize::new(0)));
    let owner = Owner::default();
    let ordinary = Grant::new(
        FileRevision::native(&cap, None).expect("read"),
        FileOperation::Read,
        owner.clone(),
        2,
        ack.clone(),
    )
    .complete(output());
    assert!(ordinary.is_error && ordinary.file_delivery.is_none());
    let recovery = Grant::new(
        FileRevision::native(&cap, None)
            .expect("read")
            .recovery()
            .expect("recovery"),
        FileOperation::Read,
        owner,
        2,
        ack,
    )
    .complete(output());
    assert!(!recovery.is_error && recovery.file_delivery.is_some());
}
