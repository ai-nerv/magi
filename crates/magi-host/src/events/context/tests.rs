#![allow(clippy::expect_used)]

use super::*;
use crate::events::{self, ClientOutcome, Emitted, Emitter, Fact};

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

#[tokio::test]
async fn nested_scopes_preserve_explicit_facts_and_process_authority() {
    let outer = Context {
        turn: Some(id(3)),
        ask: Some(id(4)),
    };
    CURRENT
        .scope(outer, async {
            assert_eq!(fill(Correlation::default()).turn_id, outer.turn);
            assert_eq!(fill(Correlation::default()).ask_id, outer.ask);
            let other = Correlation {
                turn_id: Some(id(5)),
                request_id: Some(id(6)),
                ..Correlation::default()
            };
            assert_eq!(fill(other), other);
            let explicit = Correlation {
                turn_id: outer.turn,
                ask_id: Some(id(7)),
                ..Correlation::default()
            };
            assert_eq!(fill(explicit), explicit);
            turn(Some(id(8)), async {
                tokio::task::yield_now().await;
                assert_eq!(
                    current(),
                    Context {
                        turn: Some(id(8)),
                        ask: None
                    }
                );
            })
            .await;
            turn(None, async {
                assert_eq!(current(), Context::default());
            })
            .await;
            assert_eq!(current(), outer);
            let root = Lineage::root(id(1), id(2));
            let scoped = bind(root);
            assert_eq!(scoped.turn_id, outer.turn);
            assert_eq!(scoped.ask_id, outer.ask);
            assert_eq!(bind(Lineage::default()), Lineage::default());
            let fixed = Lineage {
                turn_id: Some(id(9)),
                ..root
            };
            assert_eq!(bind(fixed), fixed);
            let (mut emitter, receiver) = Emitter::new(id(12), [1, 2, 3]);
            emitter.lineage = fixed;
            assert_eq!(emitter.emit(client), Emitted::Queued);
            let event = events::decode(&receiver.try_recv().expect("event")).expect("decode");
            assert_eq!(event.correlation.turn_id, fixed.turn_id);
            assert_eq!(event.correlation.ask_id, None);
            let fixed = Lineage {
                ask_id: Some(id(10)),
                ..scoped
            };
            assert_eq!(bind(fixed), fixed);
            let (mut emitter, receiver) = Emitter::new(id(11), [1, 2, 3]);
            emitter.lineage = fixed;
            assert_eq!(emitter.emit(client), Emitted::Queued);
            let event = events::decode(&receiver.try_recv().expect("event")).expect("decode");
            assert!(fixed.accepts(event.correlation));
        })
        .await;
    assert_eq!(current(), Context::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tasks_and_detached_work_do_not_share_context() {
    let mut tasks = Vec::new();
    for n in 1..=16 {
        let context = Context {
            turn: Some(id(n)),
            ask: Some(id(100 + n)),
        };
        tasks.push(tokio::spawn(CURRENT.scope(context, async move {
            for _ in 0..32 {
                tokio::task::yield_now().await;
                assert_eq!(current(), context);
                let (left, right) = tokio::join!(
                    turn(Some(id(200 + n)), async {
                        tokio::task::yield_now().await;
                        current()
                    }),
                    turn(Some(id(300 + n)), async {
                        tokio::task::yield_now().await;
                        current()
                    }),
                );
                assert_ne!(left.turn, right.turn);
                assert_eq!(left.ask, None);
                assert_eq!(right.ask, None);
                assert_eq!(current(), context);
            }
            assert_eq!(
                tokio::spawn(async { current() })
                    .await
                    .expect("detached task"),
                Context::default()
            );
            assert_eq!(
                tokio::task::spawn_blocking(current)
                    .await
                    .expect("blocking task"),
                Context::default()
            );
        })));
    }
    for task in tasks {
        task.await.expect("scoped task");
    }
    assert_eq!(current(), Context::default());
}

#[tokio::test]
async fn cancellation_and_unwind_restore_the_callers_scope() {
    let outer = Context {
        turn: Some(id(1)),
        ask: Some(id(2)),
    };
    CURRENT
        .scope(outer, async {
            let (sent, received) = tokio::sync::oneshot::channel();
            let future = turn(Some(id(3)), async {
                assert_eq!(current().turn, Some(id(3)));
                sent.send(()).expect("poll notification");
                std::future::pending::<()>().await;
            });
            {
                let future = std::pin::pin!(future);
                tokio::select! {
                    () = future => panic!("pending scoped future completed"),
                    result = received => result.expect("scope polled"),
                }
            }
            assert_eq!(current(), outer);
            assert!(
                crate::catching::caught(turn(Some(id(4)), async {
                    tokio::task::yield_now().await;
                    assert_eq!(current().turn, Some(id(4)));
                    panic!("scoped fixture panic");
                }))
                .await
                .is_err()
            );
            assert_eq!(current(), outer);
        })
        .await;
    assert_eq!(current(), Context::default());
}

#[test]
fn enabled_asks_bind_once_and_disabled_asks_allocate_no_identity() {
    const PROBE: &str = "MAGI_EVENT_CONTEXT_PROBE";
    if std::env::var_os(PROBE).is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "events::context::tests::enabled_asks_bind_once_and_disabled_asks_allocate_no_identity"])
            .env(PROBE, "1").output().expect("isolated probe");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        ask(async {
            assert_eq!(current(), Context::default());
            assert_eq!(lineage(), Lineage::default());
            assert_eq!(
                events::emit(|| panic!("disabled payload prepared")),
                Emitted::Disabled
            );
        })
        .await;
        let receiver = events::enable_root().expect("root activation");
        let root = events::process_lineage();
        assert!(root.run_id.is_some() && root.agent_id.is_some());
        let mut asks = Vec::new();
        turn(Some(id(7)), async {
            for _ in 0..2 {
                ask(async {
                    let bound = lineage();
                    assert_eq!(bound.run_id, root.run_id);
                    assert_eq!(bound.agent_id, root.agent_id);
                    assert_eq!(bound.turn_id, Some(id(7)));
                    asks.push(bound.ask_id.expect("ask identity"));
                    for _ in 0..3 {
                        tokio::task::yield_now().await;
                        assert_eq!(lineage(), bound);
                        assert_eq!(events::emit(client), Emitted::Queued);
                        let event =
                            events::decode(&receiver.try_recv().expect("event")).expect("decode");
                        assert!(bound.accepts(event.correlation));
                    }
                    assert_eq!(events::process_lineage(), root);
                })
                .await;
                assert_eq!(current().ask, None);
            }
        })
        .await;
        assert_ne!(asks.first(), asks.last());
        assert_eq!(lineage(), root);
        assert_eq!(current(), Context::default());
    });
}
