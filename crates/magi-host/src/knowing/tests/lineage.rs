#[cfg(test)]
mod native {
    use super::super::*;
    use crate::events::{
        self,
        collection::{Owner, bootstrap, runtime::Scope},
    };
    use crate::judging::Judge;
    use magi_tools::holding::Answers;
    use serde_json::json;
    use std::{io::Read, os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

    const CHILD: &str = "MAGI_QUEUED_LINEAGE_PROBE";
    const OUTPUT: &str = "MAGI_QUEUED_LINEAGE_OUTPUT";
    const VERDICT: &str = r#"{"safe":true,"rule":"read-only","reason":"reads fixture"}"#;

    #[test]
    fn provider() {
        let Some(path) = std::env::var_os(OUTPUT) else {
            return;
        };
        let sender = bootstrap::receive_stdin(Owner::Melchior).expect("native bootstrap");
        let mut body = Vec::new();
        std::io::stdin()
            .take(65537)
            .read_to_end(&mut body)
            .expect("request");
        assert!(body.len() <= 65536);
        std::fs::write(
            path,
            serde_json::to_vec(&sender.lineage()).expect("lineage"),
        )
        .expect("observed binding");
        println!("{}", magi_testkit::mind::text_line(VERDICT));
        println!("{}", magi_testkit::mind::stop_line());
    }

    #[test]
    fn actual_surface_and_judging_queues_preserve_source_not_dispatcher_context() {
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "knowing::tests::lineage::native::actual_surface_and_judging_queues_preserve_source_not_dispatcher_context"])
            .env(CHILD, "1").output().expect("isolated probe");
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        let scope = Scope::start().expect("collection");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), scenario())
                .await
                .expect("scenario deadline");
        });
        drop(runtime);
        drop(scope);
    }

    async fn scenario() {
        let scratch = magi_model::scratch::Scratch::new("queue-lineage", "native");
        let program = scratch.join("provider");
        let output = scratch.join("binding.json");
        let executable = magi_proto::permit::quoted(
            std::env::current_exe()
                .expect("binary")
                .to_str()
                .expect("UTF-8"),
        );
        let capability = json!({"ok":true,"family":1,"n":1,"result":[{"schema":1,"kind":"native_event_capabilities","program":"melchior","bootstrap":2}]}).to_string();
        std::fs::write(&program, format!("#!/bin/sh\nif [ \"$1\" = event-capabilities ]; then printf '%s\\n' '{capability}'; exit 0; fi\nexec {executable} --exact knowing::tests::lineage::native::provider --nocapture\n")).expect("provider wrapper");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
            .expect("executable");
        let mut catalog = crate::catalog::Catalog::empty();
        catalog.mind = program.display().to_string();
        catalog
            .environ
            .insert(OUTPUT.into(), output.display().to_string());
        catalog.cards.push(serde_json::from_value(json!({"id":"fake/model","name":"model","provider":"fake","api":"openai-completions","ready":true,"reasons":false,"vision":false})).expect("card"));
        catalog
            .helpers
            .roles
            .insert("safety".into(), "fake/model".into());
        let models = Arc::new(crate::refreshing::Models::new(catalog));
        let session = a_session();
        session.lock().await.set_model(Some(magi_proto::ModelInfo {
            name: "fake/model".into(),
            context_window: 8192,
        }));
        let root = events::process_lineage();
        let wrong = events::Id::generate().expect("unrelated turn");
        let mut asks = std::collections::HashSet::new();
        for (judge, present, expired) in [
            (false, true, false),
            (true, true, false),
            (false, false, false),
            (true, false, false),
            (false, true, true),
        ] {
            let source = present.then(|| events::Id::generate().expect("source"));
            let (sending, mut receiving) = tokio::sync::mpsc::unbounded_channel();
            let (parent_send, parent_receive) = tokio::sync::oneshot::channel();
            let source_thread = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("source runtime");
                runtime.block_on(events::context::turn(source, events::context::ask(async {
                parent_send.send(events::context::lineage().ask_id).expect("parent ask");
                if judge {
                    let helper = crate::judging::Helper::new(sending, std::path::Path::new("/private"));
                    assert!(helper.judge("read", &magi_proto::permit::Action::Read { path: "private-canary".into() }).is_some());
                } else {
                    let knows = Knows::of(&magi_proto::SessionId::new("private-session"), "/private").asking(sending);
                    let args = json!({"role":"safety","input":"private-canary","event_context":{"turn_id":wrong}});
                    let answer = if expired { knows.ask_along(Wonder::Helper, &args, Duration::from_millis(5)) } else { knows.answer(Wonder::Helper, &args) };
                    assert!(matches!(answer, Answered::Refused { .. }) == expired);
                }
            })));
            });
            let request = receiving.recv().await.expect("queued request");
            let parent_ask = parent_receive.await.expect("parent ask");
            assert_eq!(request.turn, source);
            if expired {
                tokio::time::timeout(Duration::from_secs(2), async {
                    while !source_thread.is_finished() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("source timed out");
            }
            let (sending, receiving) = tokio::sync::mpsc::unbounded_channel();
            sending.send(request).expect("dispatch queued request");
            drop(sending);
            events::context::turn(
                Some(wrong),
                serve(
                    receiving,
                    Arc::new(tokio::sync::Mutex::new(None)),
                    Arc::clone(&session),
                    Arc::clone(&models),
                ),
            )
            .await;
            let tasks = session.lock().await.helpers();
            tasks
                .pause()
                .expect("pause")
                .drain(Duration::from_secs(10))
                .await
                .expect("settle");
            source_thread.join().expect("source");
            let observed: events::Lineage =
                serde_json::from_slice(&std::fs::read(&output).expect("native result"))
                    .expect("binding");
            assert_eq!(observed.run_id, root.run_id);
            assert_eq!(observed.agent_id, root.agent_id);
            assert_eq!(observed.turn_id, source);
            let ask = observed.ask_id.expect("helper ask");
            assert_ne!(Some(ask), parent_ask);
            assert!(asks.insert(ask));
            assert_eq!(events::context::lineage(), root);
        }
    }
}
