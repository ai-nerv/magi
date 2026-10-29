use super::*;
use magi_proto::AgentStatus;

#[tokio::test]
async fn output_budget_refusal_preserves_history_policy_and_usage_without_compaction() {
    for memory_on in [false, true] {
        for counts in [
            json!({}),
            json!({"prompt_tokens":12000,"limit_tokens":16000,"requested_output_tokens":8000}),
            json!({"prompt_tokens":16000,"limit_tokens":16000}),
            json!({"prompt_tokens":u64::MAX,"limit_tokens":0,"requested_output_tokens":u64::MAX}),
        ] {
            let (session, _dir) = session("typed-output");
            let original = user("u1", &"exact input\r\n".repeat(1000));
            session
                .lock()
                .await
                .commit(original.clone())
                .expect("commit");
            let memory = Memory::start("typed-output", |_| Vec::new());
            let mut detail = counts;
            detail["kind"] = json!("output_budget");
            let failure = json!({"event":"failed","why":"overflow",
                "message":"output reservation exceeds provider limit","overflow":detail})
            .to_string();
            let usage = magi_proto::Usage {
                output: 7,
                ..Default::default()
            };
            let spent = json!({"event":"spent","usage":usage}).to_string();
            let mind = Mind::saying("typed-output", &[&text_line("partial"), &spent, &failure]);
            let mut backend = backend(&mind);
            backend.wants.max_tokens = Some(77);
            backend.wants.thinking = Some(magi_model::ThinkingLevel::Medium);
            let policy = backend.wants.clone();
            let scribe = if memory_on {
                memory.scribe().await
            } else {
                Arc::new(tokio::sync::Mutex::new(None))
            };
            let registry = magi_tools::Registry::new();
            let ops = magi_tools::ops::Real::new(std::env::temp_dir());
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                magi_host::turn::run(&session, &backend, &registry, &ops, &scribe),
            )
            .await
            .expect("bounded turn");
            let Err(magi_host::HostError::Refused(why)) = result else {
                panic!("expected actionable refusal: {result:?}");
            };
            assert!(why.contains("output reservation exceeds provider limit"));
            assert!(why.contains("Review the output allowance and thinking requirements"));
            assert_eq!(mind.asked(), 1);
            assert_eq!(memory.overflow_calls(), 0);
            assert_eq!(backend.wants, policy);
            let request: Value = serde_json::from_str(&mind.asks()[0]).expect("request");
            assert_eq!(request["wants"]["max_tokens"], 77);
            assert_eq!(request["wants"]["thinking"], "medium");
            assert_eq!(
                request["context"]["messages"][0]["content"][0]["text"],
                "exact input\r\n".repeat(1000)
            );
            let held = session.lock().await;
            assert_eq!(held.status(), &AgentStatus::Idle);
            assert_eq!(held.entries().len(), 2);
            assert_eq!(held.entries()[0], original);
            assert!(matches!(&held.entries()[1], Entry::Assistant {
                text, stop_reason:Some(StopReason::Error), error:Some(error), usage:actual, ..
            } if text == "partial" && error == "output reservation exceeds provider limit" && *actual == usage));
        }
    }
}

#[tokio::test]
async fn input_unknown_and_malformed_detail_keep_bounded_input_recovery() {
    for detail in [
        Value::Null,
        json!({"kind":"input"}),
        json!({"kind":"unknown"}),
        json!({"kind":"future"}),
        json!({"kind":"output_budget","limit_tokens":-1}),
    ] {
        let (session, _dir) = session("typed-input");
        let original = user("u1", "exact input");
        session
            .lock()
            .await
            .commit(original.clone())
            .expect("commit");
        let memory = Memory::start("typed-input", items);
        let failure = json!({"event":"failed","why":"overflow",
            "message":"context length exceeded","overflow":detail})
        .to_string();
        let mind = Mind::saying("typed-input", &[&failure]);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run(&session, &mind, &memory),
        )
        .await
        .expect("bounded turn");
        assert!(format!("{result:?}").contains("nothing left to leave out"));
        assert_eq!(mind.asked(), 1);
        assert_eq!(memory.overflow_calls(), 1);
        let held = session.lock().await;
        assert_eq!(held.status(), &AgentStatus::Idle);
        assert_eq!(held.entries().len(), 2);
        assert_eq!(held.entries()[0], original);
    }
}
