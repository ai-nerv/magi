use super::*;
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use std::os::fd::OwnedFd;

struct PausedWorker {
    pid: u32,
    fd: OwnedFd,
}

impl Drop for PausedWorker {
    fn drop(&mut self) {
        let _ = pidfd_send_signal(&self.fd, Signal::Cont);
    }
}

fn owned_worker(pid: u32, host: u32) -> bool {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let parent = status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .and_then(|text| text.trim().parse::<u32>().ok());
    let command = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let image = std::fs::read_link(format!("/proc/{pid}/exe"));
    parent == Some(host)
        && command.split(|byte| *byte == 0).nth(1) == Some(b"--image-worker-v1")
        && image.is_ok()
        && image.ok() == std::fs::read_link(format!("/proc/{host}/exe")).ok()
}

async fn pause_worker(fixture: &Fixture) -> PausedWorker {
    let host = fixture.child.0.id();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            for task in std::fs::read_dir(format!("/proc/{host}/task")).expect("owned host tasks") {
                let children = std::fs::read_to_string(task.expect("task").path().join("children"))
                    .unwrap_or_default();
                for pid in children.split_whitespace() {
                    let pid: u32 = pid.parse().expect("child PID");
                    let Some(id) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
                        continue;
                    };
                    let Ok(fd) = pidfd_open(id, PidfdFlags::empty()) else {
                        continue;
                    };
                    if !owned_worker(pid, host) {
                        continue;
                    }
                    if pidfd_send_signal(&fd, Signal::Stop).is_err() {
                        continue;
                    }
                    let paused = PausedWorker { pid, fd };
                    loop {
                        let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
                            .expect("stopped owned worker");
                        if status.lines().any(|line| line.starts_with("State:\tT")) {
                            assert!(owned_worker(pid, host), "worker identity changed");
                            return paused;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("native worker must be observed before preparation completes")
}

async fn pending(fixture: &mut Fixture, request: u64) -> PausedWorker {
    let image = picture(4000, 2000);
    magi_model::validate_images(std::slice::from_ref(&image)).expect("bounded image fixture");
    fixture
        .write
        .write(&UiCommand::SubmitTracked {
            request,
            text: "private in-flight image".into(),
            aside: String::new(),
            images: vec![image],
        })
        .await
        .expect("tracked image prompt");
    let paused = pause_worker(fixture).await;
    assert_eq!(fixture.mind.asked(), 0);
    assert!(fixture.snapshot().await.1.is_empty());
    paused
}

async fn refused(fixture: &mut Fixture, request: u64) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match next(&mut fixture.read).await {
                HarnessEvent::PromptRefused {
                    request: id,
                    message,
                } if id == request => {
                    assert!(!message.contains("private in-flight image"));
                    return message;
                }
                HarnessEvent::TurnStarted {
                    request: Some(id), ..
                } if id == request => {
                    panic!("in-flight image prompt was admitted");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("preparation refusal")
}

async fn reaped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("native host must reap the image worker");
}

async fn finished(fixture: &mut Fixture, request: u64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut started = None;
        loop {
            match next(&mut fixture.read).await {
                HarnessEvent::TurnStarted {
                    owner,
                    request: Some(id),
                } if id == request => started = Some(owner),
                HarnessEvent::TurnFinished { owner } if Some(owner) == started => return,
                HarnessEvent::PromptRefused {
                    request: id,
                    message,
                } if id == request => panic!("admitted turn refused: {message}"),
                _ => {}
            }
        }
    })
    .await
    .expect("cancelled turn lifecycle");
}

async fn send(fixture: &mut Fixture, request: u64) {
    fixture
        .write
        .write(&UiCommand::SubmitTracked {
            request,
            text: "cancel the in-flight image operation".into(),
            aside: String::new(),
            images: Vec::new(),
        })
        .await
        .expect("prompt");
}

#[tokio::test]
async fn native_interrupt_cancels_in_flight_image_preparation_and_reaps_worker() {
    let Some(mut fixture) = Fixture::start("image-preparing-cancel", 1000).await else {
        return;
    };
    let paused = pending(&mut fixture, 41).await;
    fixture
        .write
        .write(&UiCommand::Interrupt)
        .await
        .expect("interrupt");
    let error = refused(&mut fixture, 41).await;
    assert!(error.contains("cancelled") || error.contains("session or configuration changed"));
    reaped(paused.pid).await;
    assert_eq!(fixture.mind.asked(), 0);
    assert!(fixture.snapshot().await.1.is_empty());
    fixture
        .prompt(42, "after cancel", vec![picture(400, 200)])
        .await
        .expect("recovery");
    assert_eq!(fixture.mind.asked(), 2);
    fixture.stop().await;
}

#[tokio::test]
async fn native_refreshed_profile_refuses_in_flight_image_before_history_admission() {
    let Some(mut fixture) = Fixture::start("image-preparing-refresh", 1000).await else {
        return;
    };
    let paused = pending(&mut fixture, 43).await;
    cards(&fixture.root, 10, 1000);
    fixture
        .write
        .write(&UiCommand::RefreshModels)
        .await
        .expect("refresh");
    loop {
        if let HarnessEvent::ModelsRefreshed { warning, .. } = next(&mut fixture.read).await {
            assert_eq!(warning, None);
            break;
        }
    }
    pidfd_send_signal(&paused.fd, Signal::Cont).expect("continue owned worker");
    assert!(
        refused(&mut fixture, 43)
            .await
            .contains("Model image limits changed")
    );
    reaped(paused.pid).await;
    assert_eq!(fixture.mind.asked(), 0);
    assert!(fixture.snapshot().await.1.is_empty());
    fixture
        .prompt(44, "new profile", vec![picture(400, 200)])
        .await
        .expect("new profile");
    let images = saved_images(&fixture.snapshot().await.1);
    assert_eq!(images.len(), 2);
    for image in images {
        assert_eq!(
            image::load_from_memory(&STANDARD.decode(image.data).expect("base64"))
                .expect("image")
                .dimensions(),
            (10, 5)
        );
    }
    fixture.stop().await;
}

#[tokio::test]
async fn native_interrupt_during_tool_image_normalization_records_failure_and_reaps_worker() {
    let Some(mut fixture) =
        Fixture::configured_image("image-tool-cancel", 1000, None, picture(4000, 2000)).await
    else {
        return;
    };
    send(&mut fixture, 45).await;
    let paused = pause_worker(&fixture).await;
    assert_eq!(fixture.mind.asked(), 1);
    fixture
        .write
        .write(&UiCommand::Interrupt)
        .await
        .expect("interrupt tool image");
    finished(&mut fixture, 45).await;
    reaped(paused.pid).await;
    assert_eq!(
        fixture.mind.asked(),
        1,
        "cancelled tool result must not trigger another ask"
    );
    let entries = fixture.snapshot().await.1;
    let results: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool {
                result: Some(result),
                ..
            } => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert!(results[0].is_error && results[0].images.is_empty());
    assert!(
        results[0]
            .output
            .starts_with("pixels\n[Image attachments omitted:")
    );
    assert!(results[0].output.contains("cancelled"));
    fixture
        .prompt(46, "after tool cancellation", vec![])
        .await
        .expect("recovery");
    assert_eq!(fixture.mind.asked(), 2);
    fixture.stop().await;
}

#[tokio::test]
async fn native_interrupt_during_history_validation_preserves_images_and_never_dispatches() {
    let Some(mut fixture) = Fixture::start("image-history-cancel", 131072).await else {
        return;
    };
    cards(&fixture.root, 4096, 131072);
    fixture
        .write
        .write(&UiCommand::RefreshModels)
        .await
        .expect("large profile");
    loop {
        if let HarnessEvent::ModelsRefreshed { warning, .. } = next(&mut fixture.read).await {
            assert_eq!(warning, None);
            break;
        }
    }
    fixture
        .prompt(47, "original pixels", vec![picture(4000, 2000)])
        .await
        .expect("stored pixels");
    let original = saved_images(&fixture.snapshot().await.1);
    assert_eq!(original.len(), 2);
    assert_eq!(fixture.mind.asked(), 2);
    send(&mut fixture, 48).await;
    let paused = pause_worker(&fixture).await;
    assert_eq!(fixture.mind.asked(), 2);
    fixture
        .write
        .write(&UiCommand::Interrupt)
        .await
        .expect("interrupt validation");
    finished(&mut fixture, 48).await;
    reaped(paused.pid).await;
    assert_eq!(
        fixture.mind.asked(),
        2,
        "cancelled validation must not dispatch"
    );
    assert_eq!(saved_images(&fixture.snapshot().await.1), original);
    fixture
        .prompt(49, "after validation cancellation", vec![])
        .await
        .expect("recovery");
    assert_eq!(fixture.mind.asked(), 3);
    assert_eq!(sent_images(&fixture.mind.asks()[2]), original);
    fixture.stop().await;
}
