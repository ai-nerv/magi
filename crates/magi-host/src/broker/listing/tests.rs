use super::*;
use magi_model::scratch::Scratch;
use serde_json::json;

async fn pid(path: &std::path::Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            assert!(pid > 0);
            return pid;
        }
        assert!(Instant::now() < deadline, "fixture did not publish its PID");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[test]
fn basic_model_roles_need_no_refresh_or_auth_extension() {
    let card =
        json!({"id":"p/m","provider":"p","name":"m","api":"openai-completions","ready":true});
    let bytes = serde_json::to_vec(&json!({"ok":true,"result":[card]})).expect("reply");
    assert_eq!(decode(&bytes, false).expect("basic role").0.len(), 1);
    assert!(decode(&bytes, true).is_err());
    let bytes = serde_json::to_vec(&json!({"ok":true,"result":[{}]})).expect("malformed");
    assert!(decode(&bytes, false).is_err());
}

#[test]
fn oversized_model_lists_are_refused_not_truncated() {
    let card = json!({"id":"p/m","provider":"p","name":"m","api":"openai-completions"});
    let bytes =
        serde_json::to_vec(&json!({"ok":true,"result":vec![card;MODELS+1]})).expect("reply");
    assert!(
        decode(&bytes, false)
            .expect_err("count bound")
            .contains("count limit")
    );
}

#[tokio::test]
async fn output_is_bounded_and_failures_do_not_echo_provider_secrets() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "while :; do printf 'private-sentinel'; done"]);
    let error = output(command, Duration::from_secs(2), 100)
        .await
        .expect_err("byte cap");
    assert!(error.contains("byte limit"));
    assert!(!error.contains("private-sentinel"));
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf 'private-sentinel'; exit 7"]);
    assert!(
        !output(command, Duration::from_secs(2), 100)
            .await
            .expect_err("exit status")
            .contains("private-sentinel")
    );
}

#[tokio::test]
async fn deadline_and_cancellation_reap_the_owned_process() {
    for cancelled in [false, true] {
        let dir = Scratch::new("magi-model-listing", "lifecycle");
        let pidfile = dir.join("pid");
        let descendant = dir.join("descendant");
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "echo $$ > \"$1\"; (trap '' TERM; while :; do :; done) & echo $! > \"$2\"; wait",
                "listing",
            ])
            .arg(&pidfile)
            .arg(&descendant);
        let task = tokio::spawn(output(command, Duration::from_millis(200), 100));
        let owner = pid(&pidfile).await;
        let descendant = pid(&descendant).await;
        if cancelled {
            task.abort();
            assert!(task.await.expect_err("cancelled").is_cancelled());
        } else {
            assert!(
                task.await
                    .expect("task")
                    .expect_err("deadline")
                    .contains("timed out")
            );
        }
        assert!(
            !std::path::Path::new(&format!("/proc/{owner}")).exists(),
            "worker was not reaped"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while std::fs::read_link(format!("/proc/{descendant}/cwd")).is_ok() {
            assert!(Instant::now() < deadline, "descendant survived cleanup");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}
