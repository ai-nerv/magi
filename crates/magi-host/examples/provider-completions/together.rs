use super::*;

const MODELS: &[&str] = &[
    "deepseek-ai/DeepSeek-V4-Pro-0813",
    "deepseek-ai/DeepSeek-V4.1-Flash",
    "meta-models/Muse-Spark-Open-816B-A42B",
    "MiniMaxAI/MiniMax-M3",
    "moonshotai/Kimi-K3",
    "moonshotai/Kimi-K3.1-Pro",
    "nvidia/NVIDIA-Nemotron-3.5-Super-VL",
    "openai/gpt-oss-120b",
    "Qwen/Qwen3.5-9B",
    "Qwen/Qwen3.6-Plus",
    "Qwen/Qwen3.7-Max",
    "reflection/Beam-1.0-501B-A23B",
    "thinkingmachines/Inkling",
    "zai-org/GLM-5.2",
    "zai-org/GLM-5.3",
    "zai-org/GLM-5.3-Flash",
];
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

fn vision(model: &str) -> bool {
    matches!(
        model,
        "deepseek-ai/DeepSeek-V4.1-Flash"
            | "meta-models/Muse-Spark-Open-816B-A42B"
            | "MiniMaxAI/MiniMax-M3"
            | "moonshotai/Kimi-K3"
            | "moonshotai/Kimi-K3.1-Pro"
            | "nvidia/NVIDIA-Nemotron-3.5-Super-VL"
            | "Qwen/Qwen3.5-9B"
            | "thinkingmachines/Inkling"
            | "zai-org/GLM-5.3-Flash"
    )
}

fn local(root: &Path, url: &str) -> Result<()> {
    let shipped = std::fs::read_to_string(root.join("shipped-providers.lua"))?;
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"
        local original, selected = melchior.provider, false
        melchior.provider = function(id, declaration)
            if id == "together" then
                assert(declaration.base_url == "https://api.together.ai/v1" and declaration.api == "openai-completions")
                assert(declaration.auth.kind == "api-key" and #declaration.auth.vars == 1)
                assert(declaration.auth.vars[1] == "TOGETHER_API_KEY" and declaration.discovery.kind == "bundled")
                assert(declaration.catalog.source == "pi-1.6.0" and declaration.catalog.provider == id)
                assert(#declaration.catalog.models == 16 and declaration.models == nil and declaration.compat == nil)
                declaration.base_url, declaration.models = {url:?}, {{}}
                for _, key in ipairs(declaration.catalog.models) do
                    declaration.models[#declaration.models + 1] = {{id=key:sub(6),base_url={url:?}}}
                end
                original(id,declaration)
                selected = true
            end
        end
        {shipped}
        melchior.provider = original
        assert(selected,"selected provider was not declared")
        for _, id in ipairs({{"openrouter","ollama"}}) do
            original(id,{{name=id,api="openai-completions",auth={{kind="none"}}}})
        end
    "#
        ),
    )?;
    Ok(())
}

fn thinking(field: &str, text: &str, body: String) -> String {
    let mut event = json!({"choices":[{"delta":{}}]});
    event["choices"][0]["delta"][field] = json!(text);
    format!("data: {event}\n\n{body}")
}

async fn scenario(root: &Path, mind: &str, model: &str, field: &str) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    local(root, &format!("http://{}/v1", listener.local_addr()?))?;
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.cwd = root.to_path_buf();
    catalog.mind = mind.into();
    catalog.cards = magi_host::broker::cards(mind).await;
    let expected: std::collections::BTreeSet<_> =
        MODELS.iter().map(|id| format!("together/{id}")).collect();
    let actual: std::collections::BTreeSet<_> = catalog
        .cards
        .iter()
        .filter(|card| card.provider == "together")
        .map(|card| card.id.clone())
        .collect();
    assert_eq!(actual, expected);
    let mut backend = catalog
        .backend(&format!("together/{model}"))
        .ok_or("missing shipped model")?;
    backend.confine = false;
    backend.isolate = false;
    backend.wants.thinking = Some(magi_model::ThinkingLevel::High);
    backend.wants.max_tokens = Some(128);
    let scripts = vec![
        thinking(field, "tool α\n", tool_stream("tool_calls")),
        thinking(field, "answer β", text_stream()),
        thinking(field, "later γ", text_stream()),
    ];
    let worker = std::thread::spawn(move || serve_captured(listener, scripts, "/v1", None));
    let mut session = Session::recorded(SessionId::new("together"), Vec::new());
    let mut first = user("first");
    if let Entry::User { images, .. } = &mut first {
        images.push(magi_model::Image {
            data: PNG.into(),
            media_type: "image/png".into(),
        });
    }
    session.commit(first)?;
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
    for (index, (reason, usage)) in assistants.iter().enumerate() {
        assert_eq!(
            *reason,
            Some(if index == 0 {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            })
        );
        assert_eq!((usage.input, usage.output, usage.cache_read), (60, 12, 30));
    }
    let heard = worker.join().map_err(|_| "Together server panicked")??;
    assert_eq!(heard.len(), 3);
    for (headers, body) in &heard {
        assert_eq!(body["model"], model);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(
            body.get("reasoning"),
            (model != "openai/gpt-oss-120b").then_some(&json!({"enabled":true}))
        );
        assert_eq!(
            body.get("reasoning_effort").and_then(Value::as_str),
            matches!(
                model,
                "openai/gpt-oss-120b" | "deepseek-ai/DeepSeek-V4-Pro-0813"
            )
            .then_some("high")
        );
        assert!(body["tools"][0]["function"].get("strict").is_none());
        assert_eq!(body.to_string().contains(PNG), vision(model));
        for name in [
            "max_completion_tokens",
            "store",
            "thinking",
            "enable_thinking",
            "prompt_cache_key",
            "__session_id",
            "__credential_binding",
        ] {
            assert!(body.get(name).is_none(), "{model}: {name}");
        }
        for header in [
            "session_id:",
            "x-session-id:",
            "x-client-request-id:",
            "x-session-affinity:",
        ] {
            assert!(!headers.contains(header));
        }
    }
    for (_, body) in &heard[1..] {
        replay(body);
        let assistant = body["messages"]
            .as_array()
            .expect("history")
            .iter()
            .find(|message| message["tool_calls"].is_array())
            .expect("calls");
        assert_eq!(assistant[field], "tool α\n");
        assert!(
            assistant
                .get(if field == "reasoning" {
                    "reasoning_content"
                } else {
                    "reasoning"
                })
                .is_none()
        );
    }
    assert!(
        heard[2].1["messages"]
            .as_array()
            .expect("history")
            .iter()
            .any(|message| message[field] == "answer β")
    );
    println!(
        "PASS together/{model}/{field}: shipped cards, 3 native host requests, 2 Echo calls, images and later-user reasoning replay"
    );
    Ok(())
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    let (mut count, mut images) = (0, 0);
    for model in MODELS {
        for field in ["reasoning", "reasoning_content"] {
            scenario(root, mind, model, field).await?;
            count += 1;
        }
        images += usize::from(vision(model));
    }
    assert_eq!((count, images), (32, 9));
    println!(
        "PASS Together: 16 models, 32 scenarios, 96 loopback requests, 64 actual Echo executions, 9 vision models"
    );
    Ok(())
}
