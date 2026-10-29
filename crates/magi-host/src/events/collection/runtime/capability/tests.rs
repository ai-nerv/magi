#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn capability_is_bounded_exact_and_owner_specific() {
    let golden = br#"{"ok":true,"family":1,"n":1,"result":[{"schema":1,"kind":"native_event_capabilities","program":"melchior","bootstrap":2}]}"#;
    assert!(accepts(golden, Owner::Melchior));
    assert!(!accepts(golden, Owner::Casper));
    for (key, value) in [
        ("ok", serde_json::json!(false)),
        ("family", serde_json::json!(2)),
        ("n", serde_json::json!(0)),
        ("extra", serde_json::json!(true)),
    ] {
        let mut reply: serde_json::Value = serde_json::from_slice(golden).expect("golden");
        reply[key] = value;
        assert!(!accepts(
            &serde_json::to_vec(&reply).expect("encode"),
            Owner::Melchior
        ));
    }
    for (key, value) in [
        ("schema", serde_json::json!(2)),
        ("bootstrap", serde_json::json!(1)),
        ("bootstrap", serde_json::json!(3)),
        ("program", serde_json::json!("casper")),
        ("kind", serde_json::json!("unknown")),
        ("secret", serde_json::json!("private")),
    ] {
        let mut reply: serde_json::Value = serde_json::from_slice(golden).expect("golden");
        reply["result"][0][key] = value;
        assert!(!accepts(
            &serde_json::to_vec(&reply).expect("encode"),
            Owner::Melchior
        ));
    }
    let mut duplicate = golden.to_vec();
    duplicate.extend_from_slice(golden);
    assert!(!accepts(&duplicate, Owner::Melchior));
    assert!(!accepts(&[b' '; LIMIT + 1], Owner::Melchior));
}

#[test]
fn query_bounds_output_time_environment_and_reaps_its_leader() {
    let root = magi_model::scratch::Scratch::new("ecap", "bounds");
    let program = root.join("probe");
    let pid = root.join("pid");
    let golden = r#"{"ok":true,"family":1,"n":1,"result":[{"schema":1,"kind":"native_event_capabilities","program":"melchior","bootstrap":2}]}"#;
    for (body, accepted) in [
        (format!("printf '%s' '{golden}'"), true),
        (format!("printf '%s' '{golden}'; exit 7"), false),
        ("printf '%0513d' 0".into(), false),
        ("printf 'not-json'".into(), false),
        ("exec sleep 30".into(), false),
    ] {
        std::fs::write(&program, format!("#!/bin/sh\nprintf '%s' $$ > '{}'\n[ \"$#\" = 1 ] && [ \"$1\" = event-capabilities ] || exit 9\n[ -z \"${{PRIVATE_EVENT_CANARY+x}}\" ] || exit 10\n{body}\n", pid.display())).expect("script");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
            .expect("executable");
        let mut command = Command::new(&program);
        command
            .arg("serve")
            .arg("private-argument-canary")
            .env("PRIVATE_EVENT_CANARY", "secret");
        let began = Instant::now();
        assert_eq!(supported(&command, Owner::Melchior), accepted, "{body}");
        assert!(began.elapsed() < Duration::from_secs(5));
        let child = std::fs::read_to_string(&pid).expect("child PID");
        assert!(
            !std::path::Path::new(&format!("/proc/{child}")).exists(),
            "query child not reaped"
        );
    }
}
