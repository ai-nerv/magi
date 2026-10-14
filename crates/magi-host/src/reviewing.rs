use magi_ipc::{FrameReader, FrameWriter, blocking};
use magi_proto::{
    UiCommand,
    permit::{Action, Decision},
};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

pub(crate) fn forward(
    socket: &Path,
    tool: &str,
    action: &Action,
    stopped: &(dyn Fn() -> bool + Send + Sync),
) -> Decision {
    let review = || -> Result<Decision, Box<dyn std::error::Error>> {
        let stream = std::os::unix::net::UnixStream::connect(socket)?;
        // The parent's key goes to the parent's user and to nobody else.
        if !magi_ipc::peer_is_us(&stream) {
            return Ok(Decision::Deny);
        }
        stream.set_read_timeout(Some(std::time::Duration::from_secs(305)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        blocking::FrameWriter::new(&stream).write_blocking(&UiCommand::Review {
            tool: format!("subagent: {tool}"),
            action: action.clone(),
            key: magi_ipc::read_key(socket),
        })?;
        Ok(std::thread::scope(|scope| {
            let (back, answer) = std::sync::mpsc::channel();
            let reading = &stream;
            scope.spawn(move || {
                let decision = blocking::FrameReader::new(reading)
                    .read_blocking()
                    .unwrap_or(Decision::Deny);
                let _ = back.send(decision);
            });
            loop {
                if stopped() {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Decision::Deny;
                }
                match answer.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(decision) => return decision,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Decision::Deny,
                }
            }
        }))
    };
    review().unwrap_or(Decision::Deny)
}

/// Ask this session's person on a child's behalf and answer the child. A child that hangs up
/// withdraws its question, rather than leaving it open until it times out.
pub(crate) async fn review(
    approver: Arc<dyn magi_tools::approve::Approver>,
    tool: String,
    action: Action,
    mut reader: FrameReader<OwnedReadHalf>,
    mut writer: FrameWriter<OwnedWriteHalf>,
    writing: std::time::Duration,
) -> Result<(), crate::HostError> {
    let withdrawn = Arc::new(AtomicBool::new(false));
    let mut asking = {
        let withdrawn = Arc::clone(&withdrawn);
        tokio::task::spawn_blocking(move || {
            crate::asking::withdrawable(withdrawn, || approver.ask(&tool, &action))
        })
    };
    // The child sends nothing after its request, so anything read, its hang-up included, is the
    // child no longer waiting.
    let decision = tokio::select! {
        decided = &mut asking => decided.unwrap_or(Decision::Deny),
        _ = reader.read::<UiCommand>() => {
            withdrawn.store(true, Ordering::SeqCst);
            let _ = asking.await;
            return Ok(());
        }
    };
    tokio::time::timeout(writing, writer.write(&decision))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog::Catalog, session::Session};
    use magi_model::scratch::Scratch;
    use magi_proto::{
        Cursor, HarnessEvent, SessionId,
        judging::Mode,
        permit::{Lifetime, Scope},
    };

    async fn parent(name: &str, catalog: Catalog) -> (Scratch, tokio::task::JoinHandle<()>) {
        let dir = Scratch::new("review", name);
        let listener = magi_ipc::bind(&dir.join("s")).await.expect("bind");
        let host = tokio::spawn(async move {
            let _ = crate::serve_on(
                listener,
                Session::recorded(SessionId::new("parent"), Vec::new()),
                None,
                catalog,
                Some("/no-memory".into()),
            )
            .await;
        });
        (dir, host)
    }

    fn read() -> Action {
        Action::Read {
            path: "/project".into(),
        }
    }

    /// A screen attached to `socket` with its key, past the snapshot.
    async fn attached(
        socket: &Path,
    ) -> (
        magi_ipc::FrameReader<OwnedReadHalf>,
        FrameWriter<OwnedWriteHalf>,
    ) {
        let stream = magi_ipc::connect(socket).await.expect("connect");
        let (reader, writer) = stream.into_split();
        let mut writer = FrameWriter::new(writer);
        let mut reader = magi_ipc::FrameReader::new(reader);
        writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: Cursor::ZERO,
                draws: false,
                key: magi_ipc::read_key(socket),
            })
            .await
            .expect("attach");
        let _: HarnessEvent = reader.read().await.expect("snapshot");
        (reader, writer)
    }

    /// How many approvals `socket`'s session says are open.
    async fn approvals(socket: &Path) -> usize {
        let stream = magi_ipc::connect(socket).await.expect("connect");
        let (reader, writer) = stream.into_split();
        FrameWriter::new(writer)
            .write(&UiCommand::Inspect)
            .await
            .expect("inspect");
        let details: magi_proto::monitoring::AgentDetails = magi_ipc::FrameReader::new(reader)
            .read()
            .await
            .expect("details");
        details.approvals
    }

    /// A child's review of `action`, sent with `key`, its connection kept open.
    async fn reviewing(
        socket: &Path,
        key: Option<String>,
    ) -> (
        magi_ipc::FrameReader<OwnedReadHalf>,
        FrameWriter<OwnedWriteHalf>,
    ) {
        let stream = magi_ipc::connect(socket).await.expect("connect");
        let (reader, writer) = stream.into_split();
        let mut writer = FrameWriter::new(writer);
        writer
            .write(&UiCommand::Review {
                tool: "subagent: read".into(),
                action: read(),
                key,
            })
            .await
            .expect("review");
        (magi_ipc::FrameReader::new(reader), writer)
    }

    #[tokio::test]
    async fn a_child_that_hangs_up_withdraws_its_question_at_once() {
        let (dir, host) = parent("hangup", Catalog::empty()).await;
        let socket = dir.join("s");
        let (mut screen, _screen) = attached(&socket).await;
        let child = reviewing(&socket, magi_ipc::read_key(&socket)).await;
        let asked = tokio::time::timeout(std::time::Duration::from_secs(2), screen.read()).await;
        assert!(
            matches!(asked, Ok(Ok(HarnessEvent::PermissionAsked { .. }))),
            "{asked:?}"
        );
        assert_eq!(approvals(&socket).await, 1, "the question is open");

        drop(child);
        let began = std::time::Instant::now();
        while approvals(&socket).await > 0 {
            assert!(
                began.elapsed() < std::time::Duration::from_secs(2),
                "the question outlived the child that asked it"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        host.abort();
    }

    #[tokio::test]
    async fn a_review_without_the_parents_key_is_refused_and_never_asked() {
        let mut catalog = Catalog::empty();
        catalog.mode = Mode::Auto;
        let (dir, host) = parent("keyless", catalog).await;
        let socket = dir.join("s");
        let (mut screen, _screen) = attached(&socket).await;
        for key in [None, Some("0".repeat(64))] {
            let (mut answer, _child) = reviewing(&socket, key).await;
            let decision: Decision =
                tokio::time::timeout(std::time::Duration::from_secs(2), answer.read())
                    .await
                    .expect("answered")
                    .expect("decision");
            assert_eq!(decision, Decision::Deny, "auto mode would have allowed it");
        }
        let shown = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            screen.read::<HarnessEvent>(),
        )
        .await;
        assert!(shown.is_err(), "a keyless review reached the screen");
        host.abort();
    }

    #[tokio::test]
    async fn a_headless_child_reaches_the_parent_permission_prompt_and_gets_its_exact_answer() {
        use magi_tools::approve::Approver;
        for decision in [
            Decision::Deny,
            Decision::Allow {
                scope: Scope::Directory {
                    path: "/project".into(),
                },
                lifetime: Lifetime::Session,
            },
        ] {
            let (dir, host) = parent("prompt", Catalog::empty()).await;
            let socket = dir.join("s");
            let (mut reader, mut writer) = attached(&socket).await;
            let asker = crate::asking::Asker::new(
                std::sync::Arc::new(crate::asking::Pending::new()),
                Box::new(|_| panic!("a headless child cannot show its own prompt")),
                Box::new(|| Cursor::ZERO),
                Box::new(|| true),
            )
            .forwarded(Some(socket));
            let asking = tokio::task::spawn_blocking(move || asker.ask("read", &read()));
            let event: HarnessEvent =
                tokio::time::timeout(std::time::Duration::from_secs(2), reader.read())
                    .await
                    .expect("prompt timeout")
                    .expect("prompt");
            let HarnessEvent::PermissionAsked {
                id, tool, action, ..
            } = event
            else {
                panic!("permission prompt")
            };
            assert_eq!(tool, "subagent: read");
            assert_eq!(action, read());
            writer
                .write(&UiCommand::Permit {
                    id,
                    decision: decision.clone(),
                })
                .await
                .expect("answer");
            assert_eq!(asking.await.expect("join"), decision);
            host.abort();
        }
    }

    #[tokio::test]
    async fn a_review_connection_does_not_count_as_an_attached_person() {
        let (dir, host) = parent("unwatched", Catalog::empty()).await;
        let socket = dir.join("s");
        let decision =
            tokio::task::spawn_blocking(move || forward(&socket, "read", &read(), &|| false))
                .await
                .expect("join");
        assert_eq!(decision, Decision::Deny);
        host.abort();
    }

    #[tokio::test]
    async fn reviews_use_the_parent_mode_without_bypassing_deny_rules() {
        for denied in [false, true] {
            let mut catalog = Catalog::empty();
            catalog.mode = Mode::Auto;
            if denied {
                catalog.rules.deny.push(magi_proto::permit::Grant {
                    verb: "read".into(),
                    scope: Scope::Anything,
                });
            }
            let (dir, host) = parent("rules", catalog).await;
            let socket = dir.join("s");
            let decision =
                tokio::task::spawn_blocking(move || forward(&socket, "read", &read(), &|| false))
                    .await
                    .expect("join");
            assert_eq!(
                decision,
                if denied {
                    Decision::Deny
                } else {
                    Decision::Allow {
                        scope: Scope::Once,
                        lifetime: Lifetime::Session,
                    }
                }
            );
            host.abort();
        }
    }

    #[test]
    fn a_missing_parent_refuses_without_granting_anything() {
        assert_eq!(
            forward(Path::new("/no-such-parent.sock"), "read", &read(), &|| {
                false
            }),
            Decision::Deny
        );
    }

    #[test]
    fn cancellation_closes_a_forwarded_request_without_waiting_for_the_parent() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = Scratch::new("review", "cancel");
        let socket = dir.join("s");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let stopped = std::sync::Arc::new(AtomicBool::new(false));
        let stop = std::sync::Arc::clone(&stopped);
        let parent = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .expect("timeout");
            let mut reader = blocking::FrameReader::new(stream);
            let _: UiCommand = reader.read_blocking().expect("review");
            stop.store(true, Ordering::SeqCst);
            assert!(
                reader.read_blocking::<UiCommand>().is_err(),
                "child closes its request"
            );
        });
        let began = std::time::Instant::now();
        assert_eq!(
            forward(&socket, "read", &read(), &|| stopped.load(Ordering::SeqCst)),
            Decision::Deny
        );
        assert!(began.elapsed() < std::time::Duration::from_secs(2));
        parent.join().expect("join");
    }
}
