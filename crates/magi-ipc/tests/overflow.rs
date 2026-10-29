use magi_proto::ask::overflow::{Detail, Kind};
use magi_proto::ask::{Refusal, Said};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Legacy {
    Failed { message: String, why: Refusal },
}

fn terminal(detail: Value) -> Value {
    json!({"event":"failed","message":"bounded refusal","why":"overflow","overflow":detail})
}

fn decoded(value: &Value) -> Result<Vec<Said>, Box<dyn std::error::Error>> {
    let mut cbor = Vec::new();
    ciborium::into_writer(value, &mut cbor)?;
    Ok(vec![
        serde_json::from_value(value.clone())?,
        ciborium::from_reader(cbor.as_slice())?,
    ])
}

fn detail(said: Said) -> Option<Detail> {
    assert!(said.is_last());
    match said {
        Said::Failed {
            message,
            why,
            overflow,
        } => {
            assert_eq!(message, "bounded refusal");
            assert_eq!(why, Refusal::Overflow);
            overflow
        }
        _ => panic!("terminal changed"),
    }
}

#[test]
fn optional_overflow_detail_has_exact_golden_and_old_new_peer_compatibility()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = terminal(
        json!({"kind":"output_budget","prompt_tokens":12000,"limit_tokens":16000,"requested_output_tokens":8000}),
    );
    let said = Said::Failed {
        message: "bounded refusal".into(),
        why: Refusal::Overflow,
        overflow: Some(Detail {
            kind: Kind::OutputBudget,
            prompt_tokens: Some(12000),
            limit_tokens: Some(16000),
            requested_output_tokens: Some(8000),
        }),
    };
    assert_eq!(serde_json::to_value(&said).expect("golden"), expected);
    for value in decoded(&expected)? {
        assert_eq!(value, said);
    }
    let legacy = Legacy::Failed {
        message: "bounded refusal".into(),
        why: Refusal::Overflow,
    };
    assert_eq!(
        serde_json::from_value::<Legacy>(expected.clone()).expect("old JSON"),
        legacy
    );
    let mut cbor = Vec::new();
    ciborium::into_writer(&said, &mut cbor).expect("new CBOR");
    assert_eq!(
        ciborium::from_reader::<Legacy, _>(cbor.as_slice()).expect("old CBOR"),
        legacy
    );
    let mut old = expected;
    old.as_object_mut().expect("object").remove("overflow");
    for value in decoded(&old)? {
        assert_eq!(serde_json::to_value(&value).expect("old bytes"), old);
        assert_eq!(detail(value), None);
    }
    Ok(())
}

#[test]
fn malformed_optional_detail_never_hides_a_readable_terminal_failure()
-> Result<(), Box<dyn std::error::Error>> {
    for invalid in [
        Value::Null,
        json!(true),
        json!(1),
        json!("future"),
        json!([]),
        json!({}),
        json!({"kind":1}),
        json!({"kind":"input","prompt_tokens":-1}),
        json!({"kind":"input","limit_tokens":1.5}),
        json!({"kind":"input","limit_tokens":"16000"}),
        json!({"kind":"input","requested_output_tokens":false}),
        json!({"kind":"input","prompt_tokens":[1]}),
        json!({"kind":"input","limit_tokens":{"value":16000}}),
    ] {
        for said in decoded(&terminal(invalid))? {
            assert_eq!(detail(said), None);
        }
    }
    for raw in [
        r#"{"event":"failed","message":"bounded refusal","why":"overflow","overflow":{"kind":"input","kind":"output_budget"}}"#,
        r#"{"event":"failed","message":"bounded refusal","why":"overflow","overflow":{"kind":"input","limit_tokens":1,"limit_tokens":2}}"#,
        r#"{"event":"failed","message":"bounded refusal","why":"overflow","overflow":{"kind":"input","limit_tokens":18446744073709551616}}"#,
    ] {
        assert_eq!(detail(serde_json::from_str(raw).expect("terminal")), None);
    }
    Ok(())
}

#[test]
fn future_kinds_and_fields_are_conservative_and_counts_keep_unsigned_precision()
-> Result<(), Box<dyn std::error::Error>> {
    for (kind, expected) in [
        ("input", Kind::Input),
        ("output_budget", Kind::OutputBudget),
        ("future_kind", Kind::Unknown),
    ] {
        for said in decoded(&terminal(
            json!({"kind":kind,"prompt_tokens":0,"limit_tokens":u64::MAX,
            "requested_output_tokens":null,"future":{"nested":[null,{},[]]}}),
        ))? {
            assert_eq!(
                detail(said),
                Some(Detail {
                    kind: expected,
                    prompt_tokens: Some(0),
                    limit_tokens: Some(u64::MAX),
                    requested_output_tokens: None
                })
            );
        }
    }
    Ok(())
}

#[test]
fn non_json_cbor_extension_values_do_not_discard_the_failure() {
    use ciborium::Value as C;
    let word = |s: &str| C::Text(s.into());
    for invalid in [
        C::Bytes(vec![0, 255]),
        C::Map(vec![(C::Integer(1.into()), C::Null)]),
        C::Map(vec![
            (word("kind"), word("input")),
            (word("limit_tokens"), C::Bytes(vec![1])),
        ]),
    ] {
        let value = C::Map(vec![
            (word("event"), word("failed")),
            (word("message"), word("bounded refusal")),
            (word("why"), word("overflow")),
            (word("overflow"), invalid),
        ]);
        let mut bytes = Vec::new();
        ciborium::into_writer(&value, &mut bytes).expect("CBOR extension");
        assert_eq!(
            detail(ciborium::from_reader(bytes.as_slice()).expect("terminal")),
            None
        );
    }
}
