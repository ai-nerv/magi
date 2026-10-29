use super::*;
use crate::skills::{Client, client::supervised::SLOTS, wire::Operation};
use magi_tools::supplier::Tooling;
use std::os::unix::fs::PermissionsExt;

#[tokio::test(flavor = "current_thread")]
async fn blocked_verification_times_out_without_blocking_the_runtime_or_starting_a_late_helper() {
    let scratch = magi_testkit::Scratch::new("workflow", "blocked-verifier");
    let program = scratch.join("helper");
    let marker = scratch.join("spawned");
    std::fs::write(
        &program,
        format!("#!/bin/sh\nprintf started > '{}'\n", marker.display()),
    )
    .expect("script");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).expect("mode");
    let client = Client::new(
        Tooling {
            program: program.to_string_lossy().into_owned(),
            ..Tooling::default()
        },
        Default::default(),
    );
    let release = Release::new();
    let gate = Arc::clone(&release.0);
    let (started, ready) = tokio::sync::oneshot::channel();
    let until = Instant::now() + Duration::from_millis(200);
    let task = tokio::spawn(async move {
        client
            .request_verified(
                std::path::Path::new("/tmp"),
                Operation::Catalog {},
                &Cancel::default(),
                until,
                move |tooling| {
                    started.send(()).expect("verification started");
                    wait(&gate);
                    magi_tools::supplier::verified(tooling)
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), ready)
        .await
        .expect("verification did not block executor")
        .expect("ready");
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("prompt deadline")
            .expect("joined"),
        Err(Error::Timeout)
    ));
    assert!(!marker.exists());
    drop(release);
    let _all = tokio::time::timeout(Duration::from_secs(5), SLOTS.acquire_many(WORKERS as u32))
        .await
        .expect("verification reaped")
        .expect("capacity");
    assert!(!marker.exists(), "expired verification started a helper");
}

#[tokio::test(flavor = "current_thread")]
async fn supervised_native_helper_is_reaped_after_cancellation_timeout_and_caller_drop() {
    let sleep = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|path| path.join("sleep"))
        .find(|path| path.is_file())
        .expect("sleep");
    for mode in ["cancel", "timeout", "drop"] {
        let scratch = magi_testkit::Scratch::new("workflow", mode);
        let program = scratch.join("helper");
        let marker = scratch.join("pid");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s' $$ > '{}'\nexec '{}' 30\n",
                marker.display(),
                sleep.display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).expect("mode");
        let client = Client::new(
            Tooling {
                program: program.to_string_lossy().into_owned(),
                ..Tooling::default()
            },
            Default::default(),
        );
        let cancel = Cancel::default();
        let flag = cancel.clone();
        let deadline = Instant::now()
            + if mode == "timeout" {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(3)
            };
        let task = tokio::spawn(async move {
            client
                .request_verified(
                    std::path::Path::new("/tmp"),
                    Operation::Catalog {},
                    &flag,
                    deadline,
                    magi_tools::supplier::verified,
                )
                .await
        });
        let pid: i32 = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&marker)
                    && let Ok(pid) = text.parse()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("helper started");
        if mode == "drop" {
            task.abort();
            assert!(task.await.expect_err("aborted").is_cancelled());
        } else {
            if mode == "cancel" {
                cancel.request();
            }
            let result = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .expect("responsive caller")
                .expect("joined");
            assert!(
                matches!(result, Err(Error::Cancelled)) && mode == "cancel"
                    || matches!(result, Err(Error::Timeout)) && mode == "timeout",
                "{mode}: {result:?}"
            );
        }
        let _all = tokio::time::timeout(Duration::from_secs(5), SLOTS.acquire_many(WORKERS as u32))
            .await
            .expect("cleanup completed")
            .expect("capacity");
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        assert!(matches!(
            rustix::process::waitpid(
                rustix::process::Pid::from_raw(pid),
                rustix::process::WaitOptions::NOHANG
            ),
            Err(rustix::io::Errno::CHILD)
        ));
    }
}
