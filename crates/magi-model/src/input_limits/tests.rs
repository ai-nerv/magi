use super::*;
use serde_json::{Value, json};

#[test]
fn input_limits_round_trip_without_inventing_defaults() {
    for value in [
        json!({}),
        json!({"images":{}}),
        json!({
            "max_request_bytes":33554432,
            "images":{"max_per_message":20,"max_per_request":600,
                "resize":{"max_width":2000,"max_height":2000,"max_bytes":4718592,"jpeg_quality":80}}
        }),
    ] {
        let parsed: InputLimits = serde_json::from_value(value.clone()).expect("limits");
        assert_eq!(serde_json::to_value(parsed).expect("encoded"), value);
    }
}

#[test]
fn input_limits_refuse_invalid_values_and_unknown_fields_at_every_level() {
    for path in [
        "/max_request_bytes",
        "/images/max_per_message",
        "/images/max_per_request",
        "/images/resize/max_width",
        "/images/resize/max_height",
        "/images/resize/max_bytes",
        "/images/resize/jpeg_quality",
    ] {
        for invalid in [json!(0), json!(-1), json!(1.5), json!("20"), json!(true)] {
            let mut raw = json!({"max_request_bytes":1,"images":{"max_per_message":1,
                "max_per_request":1,"resize":{"max_width":1,"max_height":1,"max_bytes":1,"jpeg_quality":1}}});
            *raw.pointer_mut(path).expect("field") = invalid;
            assert!(
                serde_json::from_value::<InputLimits>(raw).is_err(),
                "{path}"
            );
        }
    }
    for invalid in [
        json!({"private-sentinel":1}),
        json!({"images":{"private-sentinel":1}}),
        json!({"images":{"resize":{"private-sentinel":1}}}),
        json!({"images":{"resize":{"jpeg_quality":101}}}),
        json!({"max_request_bytes":18446744073709551616_f64}),
        Value::Bool(false),
    ] {
        assert!(serde_json::from_value::<InputLimits>(invalid).is_err());
    }
}
