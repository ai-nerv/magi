use super::*;

const MODELS: &[&str] = &[
    "deepseek-ai/DeepSeek-V4-Flash-0731",
    "deepseek-ai/DeepSeek-V4-Pro-0813",
    "deepseek-ai/DeepSeek-V4.1-Flash",
    "deepseek-ai/DeepSeek-V4.1-Flash-1017",
    "deepseek-ai/DeepSeek-V4.1-Flash-1017-Fast",
    "deepseek-ai/DeepSeek-V4.1-Flash-Fast",
    "moonshotai/Kimi-K3",
    "moonshotai/Kimi-K3.1-Pro",
    "nvidia/NVIDIA-Nemotron-3-Ultra-550B-A55B",
    "openai/gpt-oss-120b",
    "zai-org/GLM-5.2",
    "zai-org/GLM-5.2-Fast",
    "zai-org/GLM-5.3",
    "zai-org/GLM-5.3-Fast",
    "zai-org/GLM-5.3-Flash",
];

fn local(root: &Path, url: &str) -> Result<()> {
    let shipped = std::fs::read_to_string(root.join("shipped-providers.lua"))?;
    let script = format!(
        r#"
        local original, selected = melchior.provider, false
        melchior.provider = function(id, declaration)
            if id == "baseten" then
                assert(declaration.base_url == "https://inference.baseten.co/v1")
                assert(declaration.catalog.source == "pi-1.6.0" and declaration.catalog.provider == id)
                assert(#declaration.catalog.models == 15 and #declaration.models == 1)
                declaration.base_url = {url:?}
                for _, key in ipairs(declaration.catalog.models) do
                    local name, found = key:sub(6), false
                    for _, model in ipairs(declaration.models) do
                        if model.id == name then model.base_url = {url:?}; found = true; break end
                    end
                    if not found then declaration.models[#declaration.models + 1] = {{id=name,base_url={url:?}}} end
                end
                original(id, declaration)
                selected = true
            end
        end
        {shipped}
        melchior.provider = original
        assert(selected, "selected provider was not declared")
        for _, id in ipairs({{"openrouter", "ollama"}}) do
            original(id, {{name=id,api="openai-completions",auth={{kind="none"}}}})
        end
    "#
    );
    std::fs::write(root.join("providers.lua"), script)?;
    Ok(())
}

fn thinking(text: &str, body: String) -> String {
    format!(
        "data: {}\n\n{body}",
        json!({"choices":[{"delta":{"reasoning_content":text}}]})
    )
}

fn affinity(headers: &str) -> Result<String> {
    let mut first = None;
    for header in ["session_id", "x-client-request-id", "x-session-affinity"] {
        let prefix = format!("{header}: ");
        let values: Vec<_> = headers
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix))
            .collect();
        assert_eq!(values.len(), 1);
        let value = values[0];
        assert_eq!(value.len(), 64);
        assert!(value.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if let Some(first) = first {
            assert_eq!(value, first);
        }
        first = Some(value);
    }
    assert!(!headers.contains("x-session-id:"));
    Ok(first.ok_or("missing affinity headers")?.into())
}

async fn scenario(root: &Path, mind: &str, model: &str) -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    local(root, &format!("http://{}/v1", listener.local_addr()?))?;
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.cwd = root.to_path_buf();
    catalog.mind = mind.into();
    catalog.cards = magi_host::broker::cards(mind).await;
    assert_eq!(
        catalog
            .cards
            .iter()
            .filter(|card| card.provider == "baseten")
            .count(),
        15
    );
    let mut backend = catalog
        .backend(&format!("baseten/{model}"))
        .ok_or("missing shipped model card")?;
    backend.confine = false;
    backend.isolate = false;
    assert!(backend.session_identity);
    assert_eq!(
        backend.context_window,
        Some(match model {
            "nvidia/NVIDIA-Nemotron-3-Ultra-550B-A55B" => 202_800,
            "openai/gpt-oss-120b" => 128_072,
            _ => 1_048_576,
        })
    );
    backend.wants.thinking = Some(magi_model::ThinkingLevel::High);
    backend.wants.max_tokens = Some(128);
    let scripts = vec![
        thinking("tool α\n", tool_stream("tool_calls")),
        thinking("answer β", text_stream()),
        thinking("later γ", text_stream()),
    ];
    let worker = std::thread::spawn(move || serve_captured(listener, scripts, "/v1", None));
    let mut session = Session::recorded(SessionId::new("baseten"), Vec::new());
    session.commit(user("first"))?;
    let session = tokio::sync::Mutex::new(session);
    let invoked = Rc::new(RefCell::new(Vec::new()));
    let mut registry = magi_tools::Registry::new();
    registry.register(Box::new(Echo(Rc::clone(&invoked))));
    let ops = magi_tools::ops::Real::new(root.to_path_buf());
    let scribe = Arc::new(tokio::sync::Mutex::new(None));
    tokio::time::timeout(WAIT, run(&session, &backend, &registry, &ops, &scribe)).await??;
    session.lock().await.commit(user("later"))?;
    tokio::time::timeout(WAIT, run(&session, &backend, &registry, &ops, &scribe)).await??;
    assert_eq!(
        *invoked.borrow(),
        vec![
            serde_json::from_str::<Value>(A)?,
            serde_json::from_str::<Value>(B)?
        ]
    );
    let held = session.lock().await;
    let assistants: Vec<_> = held
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Assistant {
                stop_reason, usage, ..
            } => Some((*stop_reason, *usage)),
            _ => None,
        })
        .collect();
    assert_eq!(assistants.len(), 3);
    assert_eq!(assistants[0].0, Some(StopReason::ToolUse));
    for (reason, usage) in &assistants[1..] {
        assert_eq!(*reason, Some(StopReason::EndTurn));
        assert_eq!(usage.output, 12);
        assert_eq!(usage.input, 60);
        assert_eq!(usage.cache_read, 30);
    }
    let heard = worker.join().map_err(|_| "Baseten server panicked")??;
    assert_eq!(heard.len(), 3);
    let key = affinity(&heard[0].0)?;
    for (headers, body) in &heard {
        assert_eq!(affinity(headers)?, key);
        assert_eq!(body["model"], model);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["tools"][0]["function"]["strict"], false);
        for field in [
            "store",
            "prompt_cache_key",
            "prompt_cache_retention",
            "thinking",
            "__thinking_enabled",
        ] {
            assert!(body.get(field).is_none(), "{model}/{field}");
        }
        assert_eq!(
            body.get("reasoning_effort"),
            (model != "moonshotai/Kimi-K3.1-Pro").then_some(&json!("high"))
        );
        assert_eq!(
            body.get("chat_template_args"),
            model
                .starts_with("zai-org/GLM-5.2")
                .then_some(&json!({"enable_thinking":true}))
        );
    }
    for (_, body) in &heard[1..] {
        replay(body);
        let assistant = body["messages"]
            .as_array()
            .expect("history")
            .iter()
            .find(|message| message["tool_calls"].is_array())
            .expect("tool calls");
        assert_eq!(assistant["reasoning_content"], "tool α\n");
    }
    assert!(
        heard[2].1["messages"]
            .as_array()
            .expect("history")
            .iter()
            .any(|message| message["reasoning_content"] == "answer β")
    );
    println!(
        "PASS baseten/{model}: shipped cards, three native host requests, two actual Echo executions, stable scoped affinity and later-user reasoning replay"
    );
    Ok(key)
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    let mut keys = std::collections::BTreeSet::new();
    for model in MODELS {
        assert!(keys.insert(scenario(root, mind, model).await?));
    }
    assert_eq!(keys.len(), 15);
    println!("PASS Baseten: 15 models, 45 loopback requests, 30 actual Echo executions");
    Ok(())
}
