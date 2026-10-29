use super::*;
use crate::tooling::{Call, Card, Ran};
use serde_json::json;

#[test]
fn recovery_read_presence_and_absence_wire_are_versioned_in_both_formats() {
    let wire = json!({"tool":"look","args":{"path":"missing"},"file_revision":{"version":1,"operation":"read"},"file_recovery":1});
    roundtrip(&wire).revision.validate().expect("recovery read");
    for value in [Value::Null, json!(2), json!("1"), json!(true), json!({})] {
        let mut invalid = wire.clone();
        invalid["file_recovery"] = value;
        assert!(roundtrip(&invalid).revision.validate().is_err());
    }
    let mut no_admission = wire.clone();
    no_admission
        .as_object_mut()
        .expect("call")
        .remove("file_revision");
    assert!(roundtrip(&no_admission).revision.validate().is_err());
    let result = json!({"said":"absent","revision_contract":1,"absence":{"version":1,"path_key":"a".repeat(64)}});
    let ran: Ran = serde_json::from_value(result.clone()).expect("typed absence");
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&result, &mut bytes).expect("independent producer");
    let decoded: Ran = ciborium::de::from_reader(bytes.as_slice()).expect("consumer");
    assert_eq!(ran, decoded);
    assert_eq!(
        serde_json::to_value(ran).expect("consumer encoding"),
        result
    );
}

fn roundtrip(value: &Value) -> Call {
    let call: Call = serde_json::from_value(value.clone()).expect("JSON call");
    assert_eq!(serde_json::to_value(&call).expect("JSON encoding"), *value);
    let mut encoded = Vec::new();
    ciborium::ser::into_writer(value, &mut encoded).expect("independent CBOR producer");
    let decoded: Call = ciborium::de::from_reader(encoded.as_slice()).expect("CBOR call");
    assert_eq!(call, decoded);
    call
}

#[test]
fn native_file_call_presence_survives_null_unknown_and_partial_guards_in_both_formats() {
    for field in [
        "file_revision",
        "revision_contract",
        "expected_revision",
        "mutation_id",
    ] {
        for value in [
            Value::Null,
            json!(1),
            json!("v1"),
            json!({}),
            json!([]),
            json!(false),
        ] {
            let mut wire = json!({"tool":"store","args":{"path":"source"}});
            wire[field] = value;
            let call = roundtrip(&wire);
            assert!(call.revision.requested());
            assert!(call.revision.validate().is_err());
        }
    }
    for capability in [
        json!({"version":2,"operation":"read"}),
        json!({"version":1,"operation":"unknown"}),
    ] {
        let call = roundtrip(&json!({"tool":"look","args":{},"file_revision":capability}));
        assert!(call.revision.validate().is_err());
    }
}

#[test]
fn native_file_guards_are_outside_model_arguments_and_legacy_calls_remain_unchanged() {
    let legacy = roundtrip(&json!({"tool":"legacy","args":{"path":"source"}}));
    assert!(!legacy.revision.requested());
    legacy.revision.validate().expect("legacy call");
    let mut wire = json!({"tool":"look","args":{"path":"source"},"file_revision":{"version":1,"operation":"read"}});
    roundtrip(&wire).revision.validate().expect("native read");
    wire["expected_revision"] = Value::Null;
    assert!(roundtrip(&wire).revision.validate().is_err());
    wire = json!({"tool":"change","args":{"path":"source","old":"before","new":"after"},
        "file_revision":{"version":1,"operation":"edit"},"revision_contract":1,
        "expected_revision":format!("cfr1:{}", "a".repeat(64)),"mutation_id":"logical-1"});
    let call = roundtrip(&wire);
    call.revision.validate().expect("guarded mutation");
    assert!(call.args.get("expected_revision").is_none());
    for (field, value) in [
        ("revision_contract", json!(2)),
        ("expected_revision", json!("cfr2:opaque")),
        ("mutation_id", json!("x".repeat(129))),
        ("mutation_id", json!("bad/id")),
    ] {
        let mut invalid = wire.clone();
        invalid[field] = value;
        assert!(roundtrip(&invalid).revision.validate().is_err());
    }
}

#[test]
fn native_file_cards_and_results_decode_independent_wire_without_changing_legacy_readers() {
    let card: Card = serde_json::from_value(json!({"name":"look","description":"native",
        "parameters":{},"file_revision":{"version":1,"operation":"read"}}))
    .expect("card");
    assert_eq!(
        card.file_revision.expect("positive capability").operation,
        FileOperation::Read
    );
    let value = json!({"said":"committed","revision_contract":1,
        "observations":[{"path_key":"opaque","revision":format!("cfr1:{}","b".repeat(64)),"coverage":"snapshot"}],
        "mutation":{"id":"logical-1","outcome":"committed","durability":"unconfirmed"}});
    let ran: Ran = serde_json::from_value(value.clone()).expect("producer result");
    assert_eq!(serde_json::to_value(&ran).expect("result encoding"), value);
    let mut encoded = Vec::new();
    ciborium::ser::into_writer(&value, &mut encoded).expect("CBOR");
    let decoded: Ran = ciborium::de::from_reader(encoded.as_slice()).expect("consumer");
    assert_eq!(ran, decoded);
    #[derive(Deserialize)]
    struct Old {
        said: String,
    }
    let old: Old = serde_json::from_value(value).expect("legacy consumer");
    assert_eq!(old.said, "committed");
    assert!(
        serde_json::from_value::<Ran>(json!({"said":"x","revision_contract":2}))
            .expect("unknown contract")
            .files
            .revision_contract
            == Some(2)
    );
}
