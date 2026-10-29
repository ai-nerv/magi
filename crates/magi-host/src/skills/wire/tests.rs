use super::*;
use serde_json::{Value, json};

fn request(operation: Operation) -> Request {
    Request {
        version: 1,
        root: "/fixture/skills".into(),
        operation,
    }
}

fn reply(request: &Request, payload: Value) -> Value {
    json!({"ok":true,"family":1,"n":1,"result":[{"version":1,"root":request.root,"operation":request.operation,"result":{"Ok":payload}}]})
}

fn listing() -> Value {
    json!({"kind":"catalog","data":{"entries":[],"invalid_names":0}})
}

fn entry(name: &str, metadata: Value) -> Value {
    json!({"name":name,"metadata":metadata})
}

fn rejected(request: &Request, reply: &Value) {
    assert_eq!(
        request
            .decode(&serde_json::to_vec(reply).expect("encode"))
            .expect_err("refused"),
        Error::Protocol
    );
}

#[test]
fn request_path_and_name_boundaries() {
    for root in [
        "relative",
        "/a/../b",
        "/a\0b",
        &format!("/{}", "a".repeat(4096)),
        &format!("/{}", "a/".repeat(256)),
    ] {
        let mut request = request(Operation::Catalog {});
        request.root = root.into();
        assert_eq!(request.encode(), Err(Error::Path));
    }
    for name in ["../a", "UPPER", "", &"a".repeat(65)] {
        assert!(
            request(Operation::Load { name: name.into() })
                .encode()
                .is_err()
        );
    }
    let mut request = request(Operation::Catalog {});
    assert!(request.encode().is_ok());
    request.version = 2;
    assert_eq!(request.encode(), Err(Error::Protocol));
}

#[test]
fn family_identity_and_shape_are_not_inferred() {
    let request = request(Operation::Catalog {});
    let valid = reply(&request, listing());
    assert!(
        request
            .decode(&serde_json::to_vec(&valid).expect("encode"))
            .is_ok()
    );
    for (pointer, value) in [
        ("/family", json!(0)),
        ("/family", json!(2)),
        ("/ok", json!(false)),
        ("/n", json!(0)),
        ("/result/0/version", json!(2)),
        ("/result/0/root", json!("/another")),
        ("/result/0/operation", json!({"kind":"load","name":"a"})),
        (
            "/result/0/result/Ok",
            json!({"kind":"source","data":{"text":"","sha256":""}}),
        ),
    ] {
        let mut changed = valid.clone();
        *changed.pointer_mut(pointer).expect("field") = value;
        rejected(&request, &changed);
    }
    for (key, value) in [
        ("extra", json!(0)),
        ("error", json!("failure")),
        ("fault", json!("refused")),
        ("surface", json!(1)),
    ] {
        let mut changed = valid.clone();
        changed
            .as_object_mut()
            .expect("object")
            .insert(key.into(), value);
        rejected(&request, &changed);
    }
    let mut missing = valid.clone();
    missing.as_object_mut().expect("object").remove("family");
    rejected(&request, &missing);
}

#[test]
fn duplicate_unknown_and_trailing_json_fields_fail() {
    let request = request(Operation::Catalog {});
    let valid = serde_json::to_string(&reply(&request, listing())).expect("encode");
    for bad in [
        valid.replacen("\"family\":1", "\"family\":1,\"family\":1", 1),
        valid.replace("\"kind\":\"catalog\"", "\"kind\":\"catalog\",\"extra\":0"),
        format!("{valid} {{}}"),
        "not JSON".into(),
    ] {
        assert_eq!(
            request.decode(bad.as_bytes()).expect_err("invalid"),
            Error::Protocol
        );
    }
    assert_eq!(
        request
            .decode(&vec![b' '; REPLY_MOST + 1])
            .expect_err("oversized"),
        Error::Protocol
    );
}

#[test]
fn catalog_counts_order_metadata_and_name_limits_are_enforced() {
    let request = request(Operation::Catalog {});
    for entries in [
        vec![
            entry("a", json!({"Err":"unreadable"})),
            entry("a", json!({"Err":"unreadable"})),
        ],
        vec![
            entry("z", json!({"Err":"unreadable"})),
            entry("a", json!({"Err":"unreadable"})),
        ],
        vec![entry("../escape", json!({"Err":"unreadable"}))],
        vec![entry("a", json!({"Ok":"x".repeat(METADATA_MOST+1)}))],
        vec![entry("a", json!({"Ok":"a\0b"}))],
    ] {
        rejected(
            &request,
            &reply(
                &request,
                json!({"kind":"catalog","data":{"entries":entries,"invalid_names":0}}),
            ),
        );
    }
    for count in [257_u64, u64::MAX] {
        rejected(
            &request,
            &reply(
                &request,
                json!({"kind":"catalog","data":{"entries":[],"invalid_names":count}}),
            ),
        );
    }
    let entries = vec![entry("a", json!({"Err":"unreadable"}))];
    rejected(
        &request,
        &reply(
            &request,
            json!({"kind":"catalog","data":{"entries":entries,"invalid_names":256}}),
        ),
    );
}

#[test]
fn selected_text_is_independently_hashed_and_byte_bounded() {
    use sha2::{Digest, Sha256};
    let request = request(Operation::Load { name: "a".into() });
    for (text, accepted) in [
        ("日".repeat(FILE_MOST / 3), true),
        ("x".repeat(FILE_MOST + 1), false),
        ("a\0b".into(), false),
    ] {
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        let value = reply(
            &request,
            json!({"kind":"source","data":{"text":text,"sha256":digest}}),
        );
        assert_eq!(
            request
                .decode(&serde_json::to_vec(&value).expect("encode"))
                .is_ok(),
            accepted
        );
    }
    rejected(
        &request,
        &reply(
            &request,
            json!({"kind":"source","data":{"text":"body","sha256":"0".repeat(64)}}),
        ),
    );
}

#[test]
fn typed_refusals_are_not_success_or_empty_catalogs() {
    let request = request(Operation::Catalog {});
    let mut value = reply(&request, listing());
    *value.pointer_mut("/result/0/result").expect("result") = json!({"Err":"unsafe_path"});
    assert_eq!(
        request
            .decode(&serde_json::to_vec(&value).expect("encode"))
            .expect_err("refused"),
        Error::Read(ReadFailure::UnsafePath)
    );
    *value.pointer_mut("/result/0/result").expect("result") = json!({"Err":"future_unknown"});
    rejected(&request, &value);
    let refused =
        json!({"ok":false,"family":1,"n":0,"result":[],"fault":"refused","error":"no worker"});
    assert_eq!(
        request
            .decode(&serde_json::to_vec(&refused).expect("encode"))
            .expect_err("refused"),
        Error::Helper
    );
}
