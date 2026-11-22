use super::*;

pub(super) const KEYS: &[&str] = &[
    "QWEN_TOKEN_PLAN_TEAM_API_KEY",
    "QWEN_TOKEN_PLAN_TEAM_CN_API_KEY",
    "QWEN_TOKEN_PLAN_INDIVIDUAL_API_KEY",
    "QWEN_TOKEN_PLAN_INDIVIDUAL_CN_API_KEY",
];
const PLANS: &[&str] = &[
    "qwen-token-plan",
    "qwen-token-plan-cn",
    "qwen-token-plan-individual",
    "qwen-token-plan-individual-cn",
];
const COMMON: &[&str] = &[
    "deepseek-v4-flash-0731",
    "deepseek-v4-pro",
    "deepseek-v4-pro-0813",
    "deepseek-v4.1-flash",
    "deepseek-v4.1-flash-1017",
    "glm-5.2",
    "glm-5.3",
    "qwen3.6-flash",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.8-flash",
    "qwen3.8-max",
    "qwen4-max-preview",
];
const TEAM: &[&str] = &[
    "MiniMax-M2.5",
    "glm-5",
    "glm-5.1",
    "kimi-k2.5",
    "kimi-k2.6",
    "kimi-k2.7-code",
    "qwen3.6-plus",
];
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

fn models(plan: &str) -> impl Iterator<Item = &'static str> {
    COMMON
        .iter()
        .chain(if plan.contains("individual") {
            &[]
        } else {
            TEAM
        })
        .copied()
}

fn local(root: &Path, plan: &str, primary: &str, url: &str) -> Result<()> {
    let shipped = std::fs::read_to_string(root.join("shipped-providers.lua"))?;
    let count = models(plan).count();
    let host = if plan.ends_with("-cn") {
        "qianwenaiapi.com"
    } else {
        "qwencloudapi.com"
    };
    let base = format!("https://token-plan.maas.{host}/compatible-mode/v1");
    let fallback = if plan.ends_with("-cn") {
        "QWEN_TOKEN_PLAN_CN_API_KEY"
    } else {
        "QWEN_TOKEN_PLAN_API_KEY"
    };
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"
        local original, selected = melchior.provider, false
        melchior.provider = function(id, declaration)
            if id == {plan:?} then
                assert(declaration.base_url == {base:?} and declaration.api == "openai-completions")
                assert(declaration.auth.kind == "api-key" and #declaration.auth.vars == 2)
                assert(declaration.auth.vars[1] == {primary:?} and declaration.auth.vars[2] == {fallback:?})
                assert(declaration.discovery.kind == "bundled")
                assert(declaration.compat.max_tokens_field == "max_completion_tokens")
                assert(declaration.catalog.source == "pi-1.6.0" and declaration.catalog.provider == id)
                assert(#declaration.catalog.models == {count} and declaration.models == nil)
                declaration.base_url, declaration.models = {url:?}, {{}}
                for _, key in ipairs(declaration.catalog.models) do
                    declaration.models[#declaration.models + 1] = {{id=key:sub(6),base_url={url:?}}}
                end
                original(id, declaration)
                selected = true
            end
        end
        {shipped}
        melchior.provider = original
        assert(selected, "selected provider was not declared")
        for _, id in ipairs({{"openrouter","ollama"}}) do
            original(id,{{name=id,api="openai-completions",auth={{kind="none"}}}})
        end
    "#
        ),
    )?;
    Ok(())
}

fn thinking(text: &str, body: String) -> String {
    format!(
        "data: {}\n\n{body}",
        json!({"choices":[{"delta":{"reasoning_content":text}}]})
    )
}

fn vision(model: &str) -> bool {
    model.starts_with("deepseek-v4.1-")
        || model.starts_with("kimi-")
        || model.starts_with("qwen3.6-")
        || model == "qwen3.7-plus"
        || model.starts_with("qwen3.8-")
        || model.starts_with("qwen4-")
}

async fn scenario(root: &Path, mind: &str, plan: &str, primary: &str, model: &str) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    local(
        root,
        plan,
        primary,
        &format!("http://{}/compatible-mode/v1", listener.local_addr()?),
    )?;
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.cwd = root.to_path_buf();
    catalog.mind = mind.into();
    catalog.cards = magi_host::broker::cards(mind).await;
    let expected: std::collections::BTreeSet<_> =
        models(plan).map(|id| format!("{plan}/{id}")).collect();
    let actual: std::collections::BTreeSet<_> = catalog
        .cards
        .iter()
        .filter(|card| card.provider == plan)
        .map(|card| card.id.clone())
        .collect();
    assert_eq!(actual, expected);
    let mut backend = catalog
        .backend(&format!("{plan}/{model}"))
        .ok_or("missing shipped model")?;
    backend.confine = false;
    backend.isolate = false;
    let low = model.starts_with("qwen3.8-") || model.starts_with("qwen4-");
    backend.wants.thinking = Some(if low {
        magi_model::ThinkingLevel::Low
    } else {
        magi_model::ThinkingLevel::High
    });
    backend.wants.max_tokens = Some(128);
    let scripts = vec![
        thinking("tool α\n", tool_stream("tool_calls")),
        thinking("answer β", text_stream()),
        thinking("later γ", text_stream()),
    ];
    let worker =
        std::thread::spawn(move || serve_captured(listener, scripts, "/compatible-mode/v1", None));
    let mut session = Session::recorded(SessionId::new(plan), Vec::new());
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
    let heard = worker.join().map_err(|_| "Qwen server panicked")??;
    assert_eq!(heard.len(), 3);
    let binary = model.starts_with("kimi-")
        || model.starts_with("MiniMax-")
        || model.starts_with("qwen3.6-")
        || model.starts_with("qwen3.7-");
    for (headers, body) in &heard {
        assert_eq!(body["model"], model);
        assert_eq!(body["max_completion_tokens"], 128);
        assert_eq!(body["enable_thinking"], true);
        assert_eq!(
            body.get("reasoning_effort"),
            (!binary).then_some(&json!(if low { "low" } else { "high" }))
        );
        assert_eq!(body["tools"][0]["function"]["strict"], false);
        assert_eq!(body.to_string().contains(PNG), vision(model));
        for field in [
            "max_tokens",
            "store",
            "thinking_budget",
            "thinking",
            "prompt_cache_key",
            "__session_id",
            "__credential_binding",
        ] {
            assert!(body.get(field).is_none(), "{plan}/{model}: {field}");
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
        "PASS {plan}/{model}: shipped cards, 3 native host requests, 2 Echo executions, images and later-user reasoning replay"
    );
    Ok(())
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    let (mut count, mut images) = (0, 0);
    for (plan, primary) in PLANS.iter().zip(KEYS) {
        for model in models(plan) {
            scenario(root, mind, plan, primary, model).await?;
            count += 1;
            images += usize::from(vision(model));
        }
    }
    assert_eq!((count, images), (66, 36));
    println!(
        "PASS Qwen: 4 plans, 66 models, 198 loopback requests, 132 actual Echo executions, 36 vision models"
    );
    Ok(())
}
