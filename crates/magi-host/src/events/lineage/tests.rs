use super::*;
use crate::events::*;

fn id(n: u64) -> Id {
    Id::try_from(format!("{n:032x}")).expect("opaque ID")
}

#[test]
fn lineage_requires_a_complete_root_and_rejects_content_fields() {
    for value in [
        serde_json::json!({"run_id":String::from(id(1))}),
        serde_json::json!({"agent_id":String::from(id(2))}),
        serde_json::json!({"turn_id":String::from(id(3))}),
        serde_json::json!({"ask_id":String::from(id(4))}),
    ] {
        let lineage: Lineage = serde_json::from_value(value).expect("typed fields");
        assert!(!lineage.valid());
    }
    for value in [
        serde_json::json!({"prompt":"private"}),
        serde_json::json!({"run_id":"user-content"}),
        serde_json::json!({"agent_id":17}),
    ] {
        assert!(serde_json::from_value::<Lineage>(value).is_err());
    }
    assert!(Lineage::default().valid());
    assert!(Lineage::root(id(1), id(2)).valid());
}

#[test]
fn emitter_binds_parent_lineage_without_replacing_owner_local_ids() {
    let lineage = Lineage {
        turn_id: Some(id(3)),
        ask_id: Some(id(4)),
        ..Lineage::root(id(1), id(2))
    };
    let (mut emitter, receiver) = Emitter::new(id(10), [1, 2, 3]);
    emitter.lineage = lineage;
    let local = Correlation {
        run_id: Some(id(91)),
        agent_id: Some(id(92)),
        turn_id: Some(id(93)),
        ask_id: Some(id(94)),
        request_id: Some(id(5)),
        attempt_id: Some(id(6)),
        job_id: Some(id(7)),
        tool_call_id: Some(id(8)),
    };
    assert_eq!(
        emitter.emit(|| (
            local,
            Fact::Client {
                outcome: ClientOutcome::Connected
            }
        )),
        Emitted::Queued
    );
    let event = decode(&receiver.try_recv().expect("queued event")).expect("event");
    assert!(lineage.accepts(event.correlation));
    assert_eq!(event.correlation.request_id, local.request_id);
    assert_eq!(event.correlation.attempt_id, local.attempt_id);
    assert_eq!(event.correlation.job_id, local.job_id);
    assert_eq!(event.correlation.tool_call_id, local.tool_call_id);
    assert!(!lineage.accepts(local));
    assert_eq!(Lineage::default().apply(local), local);
    let root = Lineage::root(id(1), id(2)).apply(local);
    assert_eq!(root.turn_id, local.turn_id);
    assert_eq!(root.ask_id, local.ask_id);
}
