use super::*;
use serde_json::json;

#[test]
fn only_complete_typed_refusal_frames_are_known_no_effect_not_error_message_text() {
    let refusal =
        json!({"ok":false,"family":1,"n":0,"result":[],"fault":"refused","error":"safe refusal"});
    assert!(
        rows_known(&serde_json::to_vec(&refusal).expect("frame"))
            .expect_err("refused")
            .no_effect()
    );
    for field in ["ok", "family", "n", "result", "fault", "error"] {
        let mut missing = refusal.clone();
        missing.as_object_mut().expect("object").remove(field);
        assert!(
            !rows_known(&serde_json::to_vec(&missing).expect("frame"))
                .expect_err("not a success")
                .no_effect(),
            "{field}"
        );
    }
    for (field, value) in [
        ("family", json!(2)),
        ("n", json!(1)),
        ("result", json!([{}])),
        ("fault", json!("failed")),
        ("error", json!("x".repeat(4097))),
        ("ok", json!("false")),
    ] {
        let mut invalid = refusal.clone();
        invalid[field] = value;
        assert!(
            !rows_known(&serde_json::to_vec(&invalid).expect("frame"))
                .expect_err("unknown")
                .no_effect()
        );
    }
    assert!(
        !rows_known(b"refused: this is not a frame")
            .expect_err("malformed")
            .no_effect()
    );
}
