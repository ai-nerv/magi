use super::*;

const MODELS: &[(&str, &[&str])] = &[
    (
        "moonshotai",
        &[
            "kimi-k2.6",
            "kimi-k2.7-code",
            "kimi-k2.7-code-highspeed",
            "kimi-k3",
            "kimi-k3.1-pro",
            "kimi-k3.1-pro-highspeed",
        ],
    ),
    (
        "moonshotai-cn",
        &[
            "kimi-k2.6",
            "kimi-k2.7-code",
            "kimi-k2.7-code-highspeed",
            "kimi-k3",
            "kimi-k3.1-pro",
            "kimi-k3.1-pro-highspeed",
        ],
    ),
    (
        "kimi-coding",
        &[
            "k3",
            "k3-256k",
            "k3.1-pro",
            "k3.1-pro-highspeed",
            "kimi-for-coding",
            "kimi-for-coding-highspeed",
        ],
    ),
];

fn local(root: &Path, id: &str, url: &str) -> Result<()> {
    let shipped = std::fs::read_to_string(root.join("shipped-providers.lua"))?;
    let remote = match id {
        "moonshotai" => "https://api.moonshot.ai/v1",
        "moonshotai-cn" => "https://api.moonshot.cn/v1",
        _ => "https://api.kimi.com/coding/v1",
    };
    let script = format!(
        r#"
        local original, selected = melchior.provider, false
        melchior.provider = function(id, declaration)
            if id == {id:?} then
                assert(declaration.base_url == {remote:?})
                assert(declaration.catalog.source == "pi-1.6.0" and declaration.catalog.provider == id)
                assert(#declaration.catalog.models == 6 and #declaration.models == 6)
                declaration.base_url = {url:?}
                for _, model in ipairs(declaration.models) do model.base_url = {url:?} end
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

async fn scenario(root: &Path, mind: &str, id: &str, model: &str) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let path = if id == "kimi-coding" {
        "/coding/v1"
    } else {
        "/v1"
    };
    let url = format!("http://{}{path}", listener.local_addr()?);
    local(root, id, &url)?;
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.cwd = root.to_path_buf();
    catalog.mind = mind.into();
    catalog.cards = magi_host::broker::cards(mind).await;
    let mut backend = catalog
        .backend(&format!("{id}/{model}"))
        .ok_or("missing shipped model card")?;
    backend.confine = false;
    backend.isolate = false;
    assert!(
        backend.session_identity,
        "selected role must advertise session identity"
    );
    assert_eq!(
        backend.context_window,
        Some(
            if model.starts_with("kimi-k2.")
                || matches!(model, "k3-256k" | "kimi-for-coding-highspeed")
            {
                262_144
            } else {
                1_048_576
            }
        )
    );
    backend.wants.thinking = Some(magi_model::ThinkingLevel::High);
    backend.wants.max_tokens = Some(128);
    let scripts = vec![
        thinking("tool α\n", tool_stream("tool_calls")),
        thinking("answer β", text_stream()),
        thinking("later γ", text_stream()),
    ];
    let worker = std::thread::spawn(move || serve_at(listener, scripts, path, Some("melchior")));
    let mut session = Session::recorded(SessionId::new("moonshot"), Vec::new());
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
    let heard = worker.join().map_err(|_| "Moonshot server panicked")??;
    assert_eq!(heard.len(), 3);
    for body in &heard {
        let key = body["prompt_cache_key"]
            .as_str()
            .expect("session cache key");
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(body["prompt_cache_key"], heard[0]["prompt_cache_key"]);
        assert_eq!(body["model"], model);
        assert_eq!(body["max_tokens"], 128);
        assert!(body.get("store").is_none());
        assert!(body["tools"][0]["function"].get("strict").is_none());
        if model.starts_with("kimi-k2.") || model == "kimi-for-coding-highspeed" {
            assert_eq!(
                body["thinking"],
                if model == "kimi-k2.6" {
                    json!({"type":"enabled","keep":"all"})
                } else {
                    json!({"type":"enabled"})
                }
            );
            assert!(body.get("reasoning_effort").is_none());
        } else {
            assert!(body.get("thinking").is_none());
            assert_eq!(body["reasoning_effort"], "high");
        }
    }
    for body in &heard[1..] {
        replay(body);
        let assistant = body["messages"]
            .as_array()
            .expect("history")
            .iter()
            .find(|m| m["tool_calls"].is_array())
            .expect("tool calls");
        assert_eq!(assistant["reasoning_content"], "tool α\n");
    }
    assert!(
        heard[2]["messages"]
            .as_array()
            .expect("history")
            .iter()
            .any(|m| m["reasoning_content"] == "answer β")
    );
    println!(
        "PASS {id}/{model}: shipped declaration, three native host requests, two actual Echo executions and later-user reasoning replay"
    );
    Ok(())
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    let mut total = 0;
    for (id, models) in MODELS {
        for model in *models {
            scenario(root, mind, id, model).await?;
            total += 1;
        }
    }
    assert_eq!(total, 18);
    println!("PASS Moonshot/Kimi: 18 models, 54 loopback requests, 36 actual Echo executions");
    Ok(())
}
