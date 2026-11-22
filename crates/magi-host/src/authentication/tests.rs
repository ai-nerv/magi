use super::*;
use magi_model::scratch::Scratch;
use std::os::unix::fs::PermissionsExt;

fn script(dir: &Scratch, body: &str) -> String {
    let path = dir.join("auth");
    std::fs::write(
        &path,
        format!("#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\n{body}\n"),
    )
    .expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
    magi_model::scratch::executable_ready(&path);
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn device_instructions_are_targeted_and_invalid_codes_urls_and_expirations_refuse() {
    for (url, code, expires, valid) in [
        ("https://provider.invalid/device", "ABCD-EFGH", 900, true),
        ("http://foreign.invalid", "ABCD-EFGH", 900, false),
        ("https://provider.invalid/device", "", 900, false),
        ("https://provider.invalid/device", "bad\ncode", 900, false),
        ("https://provider.invalid/device", "ABCD-EFGH", 0, false),
        ("https://provider.invalid/device", "ABCD-EFGH", 901, false),
    ] {
        let dir = Scratch::new("auth-device", "bounds");
        let device =
            serde_json::json!({"event":"device","url":url,"code":code,"expires_in":expires});
        let program = script(
            &dir,
            &format!(
                "cat >/dev/null\nprintf '%s\\n' '{}' '{{\"event\":\"finished\",\"providers\":[]}}'",
                device
            ),
        );
        let (tx, mut rx) = mpsc::channel(4);
        let success = run(
            &program,
            &BTreeMap::new(),
            Request::Login {
                provider: "fixture".into(),
            },
            42,
            tx,
        )
        .await;
        assert_eq!(success, valid);
        let reply = rx.recv().await.expect("reply");
        assert!(matches!(
            reply,
            magi_proto::HarnessEvent::Authentication { request: 42, .. }
        ));
        assert_eq!(
            matches!(
                reply,
                magi_proto::HarnessEvent::Authentication {
                    reply: Reply::Device { .. },
                    ..
                }
            ),
            valid
        );
    }
}

#[tokio::test]
async fn malformed_and_oversized_output_never_echo_secrets() {
    for body in [
        "printf 'private-secret\\n'",
        "while :; do printf private-secret; done",
        "exit 3",
    ] {
        let dir = Scratch::new("auth-bounds", "bad");
        let program = script(&dir, body);
        let (tx, mut rx) = mpsc::channel(4);
        assert!(!run(&program, &BTreeMap::new(), Request::List, 1, tx).await);
        let reply = rx.recv().await.expect("reply");
        assert!(matches!(
            &reply,
            magi_proto::HarnessEvent::Authentication {
                reply: Reply::Finished { error: Some(_), .. },
                ..
            }
        ));
        assert!(!format!("{reply:?}").contains("private-secret"));
    }
}

#[tokio::test]
async fn cancelling_authentication_reaps_child_and_its_descendants() {
    let dir = Scratch::new("auth-cancel", "lifetime");
    let pid = dir.join("pid");
    let descendant = dir.join("descendant");
    let program = script(
        &dir,
        &format!(
            "echo $$ > '{}'\n(trap '' TERM; while :; do :; done) &\necho $! > '{}'\nwait",
            pid.display(),
            descendant.display()
        ),
    );
    let (tx, _rx) = mpsc::channel(4);
    let task = tokio::spawn(async move {
        run(
            &program,
            &BTreeMap::new(),
            Request::Login {
                provider: "fixture".into(),
            },
            1,
            tx,
        )
        .await
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let read_pid = |path: &std::path::Path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    };
    while read_pid(&pid).is_none() || read_pid(&descendant).is_none() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = read_pid(&pid).expect("pid");
    let descendant = read_pid(&descendant).expect("descendant");
    task.abort();
    assert!(task.await.expect_err("cancelled").is_cancelled());
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    let state = std::fs::read_to_string(format!("/proc/{descendant}/stat"));
    assert!(
        state.is_err()
            || state
                .expect("state")
                .split(") ")
                .nth(1)
                .is_some_and(|s| s.starts_with('Z'))
    );
}
