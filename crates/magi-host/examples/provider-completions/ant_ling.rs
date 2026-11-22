use super::*;
use std::collections::BTreeSet;

const MODELS: &[&str] = &["Ling-2.6-1T", "Ling-2.6-flash", "Ring-2.6-1T"];
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

fn local(root: &Path, url: &str) -> Result<()> {
    let shipped = std::fs::read_to_string(root.join("shipped-providers.lua"))?;
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"
        local original, selected = melchior.provider, false
        melchior.provider = function(id,declaration)
            if id == "ant-ling" then
                assert(declaration.base_url == "https://api.ant-ling.com/v1" and declaration.api == "openai-completions")
                assert(declaration.auth.kind == "api-key" and #declaration.auth.vars == 1)
                assert(declaration.auth.vars[1] == "ANT_LING_API_KEY" and declaration.discovery.kind == "bundled")
                assert(declaration.catalog.source == "pi-1.6.0" and declaration.catalog.provider == id)
                assert(#declaration.catalog.models == 3 and declaration.models == nil and declaration.compat == nil)
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
    let cards: Vec<_> = catalog
        .cards
        .iter()
        .filter(|card| card.provider == "ant-ling")
        .collect();
    assert_eq!(cards.len(), 3);
    assert_eq!(
        cards
            .iter()
            .map(|card| card.id.clone())
            .collect::<BTreeSet<_>>(),
        MODELS.iter().map(|id| format!("ant-ling/{id}")).collect()
    );
    for card in cards {
        let ring = card.id == "ant-ling/Ring-2.6-1T";
        assert!(card.ready);
        assert!(!card.vision);
        assert_eq!(card.reasons, ring);
        assert_eq!(card.context_window, Some(262144));
        assert_eq!(card.max_output, Some(65536));
        assert_eq!(card.api, "openai-completions");
        let caps = card.capabilities.as_ref().ok_or("capabilities")?;
        assert_eq!(
            caps.thinking_levels,
            Some(if ring {
                vec!["high".into(), "xhigh".into()]
            } else {
                vec![]
            })
        );
        assert_eq!(caps.strict_tools, Some(true));
    }
    let mut backend = catalog
        .backend(&format!("ant-ling/{model}"))
        .ok_or("missing shipped model")?;
    backend.confine = false;
    backend.isolate = false;
    backend.wants.thinking = Some(magi_model::ThinkingLevel::XHigh);
    backend.wants.max_tokens = Some(128);
    let scripts = vec![
        thinking(field, "tool α\n", tool_stream("tool_calls")),
        thinking(field, "answer β", text_stream()),
        thinking(field, "later γ", text_stream()),
    ];
    let worker = std::thread::spawn(move || serve_captured(listener, scripts, "/v1", None));
    let mut session = Session::recorded(SessionId::new("ant-ling"), Vec::new());
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
    let heard = worker.join().map_err(|_| "Ant Ling server panicked")??;
    assert_eq!(heard.len(), 3);
    for (headers, body) in &heard {
        assert_eq!(body["model"], model);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(
            body.get("reasoning"),
            (model == "Ring-2.6-1T").then_some(&json!({"effort":"xhigh"}))
        );
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(!body.to_string().contains(PNG));
        for name in [
            "max_completion_tokens",
            "store",
            "reasoning_effort",
            "thinking",
            "enable_thinking",
            "chat_template_kwargs",
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
        "PASS ant-ling/{model}/{field}: exact shipped cards, 3 native host requests, 2 Echo calls, image fallback and later-user reasoning replay"
    );
    Ok(())
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    let mut count = 0;
    for model in MODELS {
        for field in ["reasoning", "reasoning_content"] {
            scenario(root, mind, model, field).await?;
            count += 1;
        }
    }
    assert_eq!(count, 6);
    println!(
        "PASS Ant Ling: 3 models, 6 scenarios, 18 loopback requests, 12 actual Echo executions, text-only image fallback"
    );
    Ok(())
}
