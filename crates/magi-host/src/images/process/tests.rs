use super::*;
use crate::images::{Failure, Mode};
use magi_model::{ImageResize, scratch::Scratch};

fn request() -> Request {
    Request {
        version: 1,
        mode: Mode::Normalize,
        profile: ImageResize::default(),
        images: Vec::new(),
    }
}

fn script(source: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(format!(
            "cat=$1; head=$2; sleep=$3; touch=$4; shift 4; {source}"
        ))
        .arg("fixture");
    for program in ["cat", "head", "sleep", "touch"] {
        let path = std::env::split_paths(&std::env::var_os("PATH").expect("test PATH"))
            .map(|directory| directory.join(program))
            .find(|path| path.is_file())
            .expect("fixture utility");
        command.arg(path);
    }
    command
}

#[tokio::test]
async fn clean_environment_and_working_directory_reach_the_child() {
    let command = script(
        "test -z \"$MAGI_IMAGE_SECRET\" && test \"$PWD\" = / || exit 1; \"$cat\" >/dev/null || exit 2; printf '%s' '{\"event\":\"ok\",\"version\":1,\"images\":[]}'",
    );
    let mut command = command;
    command.env("MAGI_IMAGE_SECRET", "must-not-reach-worker");
    assert!(
        run_with(command, request(), Duration::from_secs(5))
            .await
            .expect("clean child")
            .is_empty()
    );
}

#[tokio::test]
async fn bounded_protocol_refuses_floods_bad_json_and_exit_failure() {
    for (source, expected) in [
        (
            "\"$cat\" >/dev/null; \"$head\" -c 1048577 /dev/zero",
            "byte limit",
        ),
        (
            "\"$cat\" >/dev/null; printf 'private-bad-data'",
            "Invalid image-worker reply",
        ),
        (
            "\"$cat\" >/dev/null; exit 9",
            "stopped without a valid result",
        ),
        (
            "\"$cat\" >/dev/null; kill -KILL $$",
            "stopped without a valid result",
        ),
    ] {
        let error = run_with(script(source), request(), Duration::from_secs(5))
            .await
            .expect_err("refuse");
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("private-bad-data"));
    }
}

#[tokio::test]
async fn request_limit_refuses_before_spawning() {
    let dir = Scratch::new("magi-images", "oversized");
    let marker = dir.join("spawned");
    let mut command = script("\"$touch\" \"$1\"");
    command.arg(&marker);
    let mut request = request();
    request.images.push(Image {
        data: "A".repeat(WIRE_BYTES),
        media_type: "image/png".into(),
    });
    let error = run_with(command, request, Duration::from_secs(5))
        .await
        .expect_err("request too large");
    assert!(error.contains("request exceeds"));
    assert!(!marker.exists());
}

#[tokio::test]
async fn waiting_for_the_worker_budget_is_deadlined_before_spawn() {
    let permits = WORKERS.acquire_many(2).await.expect("all worker slots");
    let dir = Scratch::new("magi-images", "worker-budget");
    let marker = dir.join("spawned");
    let mut command = script("\"$touch\" \"$1\"");
    command.arg(&marker);
    let error = run_with(command, request(), Duration::from_millis(25))
        .await
        .expect_err("worker budget deadline");
    assert_eq!(error, "Image normalization timed out.");
    assert!(!marker.exists());
    drop(permits);
}

async fn started(path: &std::path::Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(path)
                && let Ok(pid) = value.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("worker announced PID")
}

#[tokio::test]
async fn cancellation_kills_and_reaps_the_owned_child_before_returning() {
    let dir = Scratch::new("magi-images", "cancel");
    let pidfile = dir.join("pid");
    let mut command = script("printf '%s' $$ >\"$1\"; exec \"$sleep\" 60");
    command.arg(&pidfile);
    let task = tokio::spawn(run_with(command, request(), Duration::from_secs(30)));
    let pid = started(&pidfile).await;
    task.abort();
    assert!(task.await.expect_err("cancelled").is_cancelled());
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "worker was not reaped"
    );
}

#[tokio::test]
async fn deadline_kills_and_reaps_a_worker_that_never_replies() {
    let dir = Scratch::new("magi-images", "deadline");
    let pidfile = dir.join("pid");
    let mut command = script("printf '%s' $$ >\"$1\"; exec \"$sleep\" 60");
    command.arg(&pidfile);
    let task = tokio::spawn(run_with(command, request(), Duration::from_secs(2)));
    let pid = started(&pidfile).await;
    assert_eq!(
        task.await.expect("task").expect_err("deadline"),
        "Image normalization timed out."
    );
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn replies_reject_foreign_versions_counts_extra_fields_and_oversized_images() {
    for value in [
        serde_json::json!({"event":"ok","version":2,"images":[]}),
        serde_json::json!({"event":"ok","version":1,"images":[],"private":"secret"}),
        serde_json::json!({"event":"ok","version":1,"images":[{"data":"AAAA","media_type":"image/png"}]}),
        serde_json::json!({"event":"failed","version":1,"error":"private-secret"}),
    ] {
        let error =
            reply(&serde_json::to_vec(&value).expect("JSON"), 0).expect_err("invalid reply");
        assert!(!error.contains("secret"));
    }
    let failed = serde_json::to_vec(&Reply::Failed {
        version: 1,
        error: Failure::History,
    })
    .expect("reply");
    assert_eq!(
        reply(&failed, 1).expect_err("history refusal"),
        Failure::History.message()
    );
    let oversized = serde_json::json!({"event":"ok","version":1,"images":[{"data":"A".repeat(WIRE_BYTES),"media_type":"image/png"}]});
    assert!(reply(&serde_json::to_vec(&oversized).expect("JSON"), 1).is_err());
}
