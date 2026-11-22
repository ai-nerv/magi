use super::*;
use magi_ipc::Wire;
use serde_json::{Value, json};

fn limits() -> Value {
    json!({"max_request_bytes":33554432,"images":{"max_per_message":20,"max_per_request":600,
        "resize":{"max_width":2000,"max_height":2000,"max_bytes":4718592,"jpeg_quality":80}}})
}

fn card() -> Value {
    json!({"id":"fixture/m","provider":"fixture","name":"M","api":"anthropic-messages",
        "ready":true,"vision":true,"capabilities":{"input":["text","image"],"input_limits":limits()}})
}

#[test]
fn input_limits_survive_listing_and_both_wire_encodings() {
    let raw = serde_json::to_vec(&json!({"ok":true,"result":[card()]})).expect("listing");
    let (cards, _) = decode(&raw, false).expect("cards");
    assert_eq!(
        serde_json::to_value(&cards[0]).expect("card")["capabilities"]["input_limits"],
        limits()
    );
    for wire in [Wire::Json, Wire::Cbor] {
        let bytes = wire.write(&cards[0]).expect("card");
        let roundtrip: Card = Wire::read(&bytes).expect("card");
        assert_eq!(
            serde_json::to_value(roundtrip).expect("card")["capabilities"]["input_limits"],
            limits()
        );
    }
}

#[test]
fn old_model_cards_keep_limits_absent_and_invalid_limits_refuse_the_entire_list() {
    let mut old = card();
    old.as_object_mut().expect("card").remove("capabilities");
    let bytes = serde_json::to_vec(&json!({"ok":true,"result":[old]})).expect("reply");
    let (cards, _) = decode(&bytes, false).expect("legacy card");
    assert!(cards[0].capabilities.is_none());
    for value in [
        json!({"max_request_bytes":0}),
        json!({"images":{"resize":{"jpeg_quality":101}}}),
        json!({"private-sentinel":1}),
    ] {
        let mut invalid = card();
        invalid["capabilities"]["input_limits"] = value;
        let bytes =
            serde_json::to_vec(&json!({"ok":true,"result":[card(),invalid]})).expect("reply");
        let error = decode(&bytes, false).expect_err("atomic refusal");
        assert!(!error.contains("private-sentinel"));
        assert!(error.contains("keeping the previous list"));
    }
}
