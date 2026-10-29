#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use magi_proto::SessionId;

fn identity(name: &str, generation: u64) -> Identity {
    Identity {
        external: name.into(),
        native: SessionId::new(name),
        generation,
    }
}

fn open(permissions: &mut Permissions, identity: &Identity, approval: &str) -> Vec<Value> {
    permissions
        .open(
            identity,
            ToolCallId::new(approval),
            "read".into(),
            Action::Read {
                path: "/selected/note".into(),
            },
            vec![
                Scope::Once,
                Scope::Directory {
                    path: "/selected".into(),
                },
            ],
            None,
        )
        .expect("permission")
}

fn request(frames: &[Value]) -> RequestId {
    serde_json::from_value(frames[1]["id"].clone()).expect("request id")
}

fn choice(id: &str) -> std::result::Result<Value, Value> {
    Ok(json!({"outcome":{"outcome":"selected", "optionId":id}}))
}

#[test]
fn permissions_preserve_native_scopes_and_session_lifetime_with_official_schema() {
    let mut permissions = Permissions::default();
    let identity = identity("session", 4);
    let frames = open(&mut permissions, &identity, "native-approval");
    let _: agent_client_protocol_schema::v1::SessionNotification =
        serde_json::from_value(frames[0]["params"].clone()).expect("official update");
    let official: RequestPermissionRequest =
        serde_json::from_value(frames[1]["params"].clone()).expect("official permission request");
    assert_eq!(official.options.len(), 3);
    assert_eq!(official.options[0].kind, PermissionOptionKind::RejectOnce);
    assert!(official.options[2].name.contains("this session only"));
    let resolution = permissions
        .answer(&request(&frames), &choice("allow-1"))
        .expect("answer")
        .expect("pending");
    assert!(resolution.identity == identity);
    assert!(
        matches!(resolution.command, UiCommand::Permit { id, decision: Decision::Allow {
        scope: Scope::Directory { path }, lifetime: Lifetime::Session,
    }} if id == ToolCallId::new("native-approval") && path == "/selected")
    );
    assert_eq!(resolution.frame["params"]["update"]["status"], "completed");
    assert!(
        permissions
            .answer(&request(&frames), &choice("allow-1"))
            .is_err()
    );
}

#[test]
fn permission_answers_cannot_select_unoffered_scopes_or_add_effects() {
    for value in [
        json!({"outcome":{"outcome":"selected", "optionId":"allow-99"}}),
        json!({"outcome":{"outcome":"selected", "optionId":"allow-0", "lifetime":"always"}}),
        json!({"outcome":{"outcome":"selected", "optionId":"allow-0"},"sessionId":"other"}),
        json!({"outcome":{"outcome":"cancelled", "optionId":"allow-0"}}),
        json!({"outcome":{"outcome":"selected", "optionId":null}}),
        json!({"outcome":{"outcome":"allow_always"}}),
        json!({"outcome":{"outcome":"cancelled"},"_meta":{"oversized":"x".repeat(8192)}}),
    ] {
        let mut permissions = Permissions::default();
        let frames = open(&mut permissions, &identity("session", 1), "approval");
        assert!(permissions.answer(&request(&frames), &Ok(value)).is_err());
        assert_eq!(permissions.pending.len(), 1);
    }
}

#[test]
fn cancellation_retires_only_the_exact_session_generation_and_never_grants_late_answers() {
    let mut permissions = Permissions::default();
    let old = identity("session", 1);
    let current = identity("session", 2);
    let a = open(&mut permissions, &old, "approval");
    let b = open(&mut permissions, &current, "approval");
    let c = open(&mut permissions, &identity("other", 1), "approval");
    let retired = permissions.retire(&old);
    assert_eq!(retired.len(), 1);
    assert!(matches!(
        retired[0].command,
        UiCommand::Permit {
            decision: Decision::Deny,
            ..
        }
    ));
    assert!(
        permissions
            .answer(&request(&a), &choice("allow-1"))
            .expect("late answer")
            .is_none()
    );
    assert!(
        permissions
            .answer(&request(&a), &choice("allow-1"))
            .is_err()
    );
    assert!(
        permissions
            .answer(&request(&b), &choice("allow-0"))
            .expect("current")
            .is_some()
    );
    assert!(
        permissions
            .answer(&request(&c), &choice("deny"))
            .expect("other")
            .is_some()
    );
}

#[tokio::test(start_paused = true)]
async fn permission_expiry_denies_and_does_not_depend_on_dispatch_selection_order() {
    let mut permissions = Permissions::default();
    let a = open(&mut permissions, &identity("a", 1), "approval");
    let b = open(&mut permissions, &identity("b", 1), "approval");
    tokio::time::advance(PATIENCE).await;
    let answer = permissions
        .answer(&request(&a), &choice("allow-0"))
        .expect("expired")
        .expect("deny");
    assert!(matches!(
        answer.command,
        UiCommand::Permit {
            decision: Decision::Deny,
            ..
        }
    ));
    assert_eq!(permissions.expire().len(), 1);
    assert!(permissions.deadline().is_none());
    assert!(
        permissions
            .answer(&request(&b), &choice("allow-1"))
            .expect("late")
            .is_none()
    );
}

#[test]
fn permission_queue_and_tombstones_are_bounded_and_evicted_ids_never_reused() {
    let mut permissions = Permissions::default();
    let identity = identity("session", 1);
    for i in 0..PENDING {
        open(&mut permissions, &identity, &format!("approval-{i}"));
    }
    assert!(
        permissions
            .open(
                &identity,
                ToolCallId::new("overflow"),
                "read".into(),
                Action::Read { path: "/a".into() },
                vec![Scope::Once],
                None
            )
            .is_err()
    );
    permissions.retire(&identity);
    let oldest = permissions.retired[0].clone();
    for _ in 0..RETIRED {
        open(&mut permissions, &identity, "same-native-id");
        permissions.retire(&identity);
    }
    assert_eq!(permissions.retired.len(), RETIRED);
    assert!(permissions.answer(&oldest, &choice("allow-0")).is_err());
    assert!(permissions.pending.is_empty());
}

#[test]
fn client_cancelled_or_error_responses_deny_without_persistent_grants() {
    for response in [
        Ok(json!({"outcome":{"outcome":"cancelled"}})),
        Err(json!({"code":-32601,"message":"unsupported"})),
        choice("deny"),
    ] {
        let mut permissions = Permissions::default();
        let frames = open(&mut permissions, &identity("session", 1), "approval");
        let answer = permissions
            .answer(&request(&frames), &response)
            .expect("deny")
            .expect("pending");
        assert!(matches!(
            answer.command,
            UiCommand::Permit {
                decision: Decision::Deny,
                ..
            }
        ));
        assert_eq!(answer.frame["params"]["update"]["status"], "failed");
    }
}
