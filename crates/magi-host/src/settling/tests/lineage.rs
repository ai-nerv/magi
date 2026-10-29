use crate::events::context::{ask, lineage, turn};
use crate::events::{self, ClientOutcome, Correlation, Emitted, Fact, Id, Lineage};
use crate::settling::Tasks;
use std::time::Duration;

fn id(n: u64) -> Id {
    Id::try_from(format!("{n:032x}")).expect("opaque identity")
}

fn client() -> (Correlation, Fact) {
    (
        Correlation::default(),
        Fact::Client {
            outcome: ClientOutcome::Connected,
        },
    )
}

#[test]
fn session_helpers_keep_the_admitting_turn_across_runtimes_without_reusing_asks() {
    const PROBE: &str = "MAGI_HELPER_LINEAGE_PROBE";
    if std::env::var_os(PROBE).is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "settling::tests::lineage::session_helpers_keep_the_admitting_turn_across_runtimes_without_reusing_asks"])
            .env(PROBE, "1").output().expect("isolated probe");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let worker = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("worker runtime");
    let session = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("session runtime");
    let tasks = Tasks::on(session.handle().clone());
    worker.block_on(async {
        tasks
            .spawn(async {
                ask(async {
                    assert_eq!(lineage(), Lineage::default());
                    assert_eq!(
                        events::emit(|| panic!("disabled payload")),
                        Emitted::Disabled
                    );
                })
                .await;
                Ok(())
            })
            .expect("disabled helper");
        tasks
            .pause()
            .expect("pause disabled")
            .drain(Duration::from_secs(5))
            .await
            .expect("disabled drained");
        let receiver = events::enable_root().expect("enable");
        let root = events::process_lineage();
        let mut releases = Vec::new();
        for n in 1..=16 {
            let (send, receive) = tokio::sync::oneshot::channel();
            turn(
                Some(id(n)),
                ask(async {
                    let parent = lineage().ask_id.expect("parent ask");
                    tasks
                        .spawn(async move {
                            receive.await.map_err(|why| why.to_string())?;
                            assert_eq!(lineage().turn_id, Some(id(n)));
                            assert_eq!(lineage().ask_id, None);
                            for _ in 0..2 {
                                ask(async {
                                    tokio::task::yield_now().await;
                                    assert_eq!(lineage().run_id, root.run_id);
                                    assert_eq!(lineage().agent_id, root.agent_id);
                                    assert_eq!(lineage().turn_id, Some(id(n)));
                                    assert_ne!(lineage().ask_id, Some(parent));
                                    assert!(lineage().ask_id.is_some());
                                    assert_eq!(events::emit(client), Emitted::Queued);
                                })
                                .await;
                                assert_eq!(lineage().ask_id, None);
                            }
                            Ok(())
                        })
                        .expect("helper accepted");
                }),
            )
            .await;
            releases.push(send);
        }
        turn(Some(id(999)), async {
            for send in releases {
                send.send(()).expect("release");
            }
            tasks
                .pause()
                .expect("pause")
                .drain(Duration::from_secs(5))
                .await
                .expect("helpers drained");
            assert_eq!(lineage().turn_id, Some(id(999)));
        })
        .await;
        let mut counts = std::collections::BTreeMap::new();
        let mut asks = std::collections::HashSet::new();
        while let Ok(bytes) = receiver.try_recv() {
            let event = events::decode(&bytes).expect("event");
            assert!(root.accepts(event.correlation));
            let turn = String::from(event.correlation.turn_id.expect("originating turn"));
            *counts.entry(turn).or_insert(0) += 1;
            assert!(asks.insert(event.correlation.ask_id.expect("unique helper ask")));
        }
        assert_eq!(asks.len(), 32);
        for n in 1..=16 {
            assert_eq!(counts.remove(&String::from(id(n))), Some(2));
        }
        assert!(counts.is_empty());
        assert_eq!(lineage(), root);
    });
}
