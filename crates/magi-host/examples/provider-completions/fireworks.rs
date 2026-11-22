use super::*;

const ANTHROPIC: &[&str] = &[
    "models/deepseek-v4p1-flash",
    "models/deepseek-v4p1-flash-1017",
    "models/ember-1",
    "models/minimax-m3",
    "models/minimax-m3p1-flash",
    "models/muse-spark-open",
    "models/qwen3p8-2p4t-a95b",
    "models/qwen3p8-max",
    "routers/deepseek-flash-latest",
    "routers/deepseek-v4p1-flash-1017-spot",
    "routers/deepseek-v4p1-flash-1017-ultrafast",
    "routers/deepseek-v4p1-flash-spot",
    "routers/kimi-fast-latest",
    "routers/kimi-latest",
    "routers/minimax-latest",
    "routers/qwen-max-latest",
];
const CHAT: &[&str] = &[
    "models/glm-5p3",
    "models/glm-5p3-flash",
    "models/kimi-k3",
    "models/kimi-k3p1-pro",
    "routers/glm-5p3-fast",
    "routers/glm-5p3-spot",
    "routers/glm-fast-latest",
    "routers/glm-flash-latest",
    "routers/glm-latest",
    "routers/kimi-k3-fast",
    "routers/kimi-k3-spot",
];
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

fn local(root: &Path, url: &str) -> Result<()> {
    let shipped = std::fs::read_to_string(root.join("shipped-providers.lua"))?;
    let anthropic = ANTHROPIC
        .iter()
        .map(|id| format!("[\"accounts/fireworks/{id}\"]=true"))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"
        local original, selected = melchior.provider, false
        local anthropic = {{{anthropic}}}
        melchior.provider = function(id, declaration)
            if id == "fireworks" then
                assert(declaration.base_url == "https://api.fireworks.ai/inference")
                assert(declaration.api == "anthropic-messages")
                assert(declaration.auth.kind == "api-key" and declaration.auth.vars[1] == "FIREWORKS_API_KEY")
                assert(declaration.discovery.kind == "bundled")
                assert(declaration.catalog.source == "pi-1.6.0" and declaration.catalog.provider == id)
                assert(#declaration.catalog.models == 27)
                assert(declaration.models == nil)
                declaration.base_url, declaration.models = {url:?}, {{}}
                for _, key in ipairs(declaration.catalog.models) do
                    local name = key:sub(6)
                    declaration.models[#declaration.models + 1] = {{id=name,base_url=anthropic[name] and {url:?} or {url:?} .. "/v1"}}
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

fn event(name: &str, value: Value) -> String {
    format!("event: {name}\ndata: {value}\n\n")
}

fn response(anthropic: bool, tools: bool, thinking: &str) -> Result<String> {
    if !anthropic {
        return Ok(format!(
            "data: {}\n\n{}",
            json!({"choices":[{"delta":{"reasoning_content":thinking}}]}),
            if tools {
                tool_stream("tool_calls")
            } else {
                text_stream()
            }
        ));
    }
    let mut out = event(
        "message_start",
        json!({"message":{"role":"assistant","content":[],"usage":{
        "input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
    );
    let mut blocks = vec![json!({"type":"thinking","thinking":thinking,"signature":""})];
    if tools {
        blocks.push(json!({"type":"tool_use","id":"a","name":"echo","input":serde_json::from_str::<Value>(A)?}));
        blocks.push(json!({"type":"tool_use","id":"b","name":"echo","input":serde_json::from_str::<Value>(B)?}));
    } else {
        blocks.push(json!({"type":"text","text":"done"}));
    }
    for (index, block) in blocks.into_iter().enumerate() {
        out += &event(
            "content_block_start",
            json!({"index":index,"content_block":block}),
        );
        out += &event("content_block_stop", json!({"index":index}));
    }
    out += &event(
        "message_delta",
        json!({"delta":{"stop_reason":if tools { "tool_use" } else { "end_turn" }},
        "usage":{"input_tokens":60,"output_tokens":12,"cache_read_input_tokens":30,"cache_creation_input_tokens":0}}),
    );
    out += &event("message_stop", json!({}));
    Ok(out)
}

fn serve(
    listener: TcpListener,
    scripts: Vec<String>,
    anthropic: bool,
) -> Result<Vec<(String, Value)>> {
    let path = if anthropic {
        "/inference/v1/messages"
    } else {
        "/inference/v1/chat/completions"
    };
    let auth = if anthropic {
        ("x-api-key", "private-sentinel")
    } else {
        ("authorization", "bearer private-sentinel")
    };
    let mut heard = Vec::new();
    for body in scripts {
        let deadline = Instant::now() + WAIT;
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => return Err(error.into()),
            }
        };
        heard.push(request_at(&mut socket, path, auth, None)?);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        socket.flush()?;
    }
    Ok(heard)
}

fn affinity(headers: &str, anthropic: bool) -> Result<String> {
    let key = headers
        .lines()
        .find_map(|line| line.strip_prefix("x-session-affinity: "))
        .ok_or("missing affinity")?;
    assert_eq!(key.len(), 64);
    assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    for name in ["session_id", "x-client-request-id"] {
        assert_eq!(
            headers
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{name}: "))),
            (!anthropic).then_some(key)
        );
    }
    assert!(!headers.contains("x-session-id:"));
    Ok(key.into())
}

fn replay_anthropic(body: &Value) -> Result<()> {
    let messages = body["messages"].as_array().ok_or("history")?;
    let blocks: Vec<_> = messages
        .iter()
        .flat_map(|m| m["content"].as_array().into_iter().flatten())
        .collect();
    assert!(
        blocks.iter().any(|b| b["type"] == "thinking"
            && b["thinking"] == "tool α\n"
            && b["signature"] == "")
    );
    for (id, args) in [("a", A), ("b", B)] {
        let tool = blocks
            .iter()
            .find(|b| b["type"] == "tool_use" && b["id"] == id)
            .ok_or("tool call")?;
        assert_eq!(tool["name"], "echo");
        assert_eq!(tool["input"], serde_json::from_str::<Value>(args)?);
        let result = blocks
            .iter()
            .find(|b| b["type"] == "tool_result" && b["tool_use_id"] == id)
            .ok_or("tool result")?;
        assert_eq!(result["content"], args);
    }
    Ok(())
}

async fn scenario(root: &Path, mind: &str, id: &str, anthropic: bool) -> Result<String> {
    let model = format!("accounts/fireworks/{id}");
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    local(
        root,
        &format!("http://{}/inference", listener.local_addr()?),
    )?;
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.cwd = root.to_path_buf();
    catalog.mind = mind.into();
    catalog.cards = magi_host::broker::cards(mind).await;
    let expected: std::collections::BTreeSet<_> = ANTHROPIC
        .iter()
        .chain(CHAT)
        .map(|id| format!("fireworks/accounts/fireworks/{id}"))
        .collect();
    let actual: std::collections::BTreeSet<_> = catalog
        .cards
        .iter()
        .filter(|card| card.provider == "fireworks")
        .map(|card| card.id.clone())
        .collect();
    assert_eq!(actual, expected);
    let mut backend = catalog
        .backend(&format!("fireworks/{model}"))
        .ok_or("missing model")?;
    backend.confine = false;
    backend.isolate = false;
    assert!(backend.session_identity);
    backend.wants.thinking = Some(magi_model::ThinkingLevel::High);
    backend.wants.max_tokens = Some(128);
    let scripts = vec![
        response(anthropic, true, "tool α\n")?,
        response(anthropic, false, "answer β")?,
        response(anthropic, false, "later γ")?,
    ];
    let worker = std::thread::spawn(move || serve(listener, scripts, anthropic));
    let mut session = Session::recorded(SessionId::new("fireworks"), Vec::new());
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
    let heard = worker.join().map_err(|_| "Fireworks server panicked")??;
    assert_eq!(heard.len(), 3);
    let key = affinity(&heard[0].0, anthropic)?;
    let vision = !matches!(
        id,
        "models/minimax-m3"
            | "routers/minimax-latest"
            | "models/qwen3p8-2p4t-a95b"
            | "models/glm-5p3"
            | "routers/glm-5p3-fast"
            | "routers/glm-5p3-spot"
            | "routers/glm-fast-latest"
            | "routers/glm-latest"
    );
    for (index, (headers, body)) in heard.iter().enumerate() {
        assert_eq!(affinity(headers, anthropic)?, key);
        assert_eq!(body["model"], model);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body.to_string().contains(PNG), vision);
        for field in [
            "prompt_cache_key",
            "prompt_cache_retention",
            "__session_id",
            "__credential_binding",
        ] {
            assert!(body.get(field).is_none());
        }
        if anthropic {
            let supported = !id.contains("qwen");
            assert_eq!(
                body.get("thinking"),
                supported.then_some(&json!({"type":"adaptive"}))
            );
            assert_eq!(
                body.get("output_config"),
                supported.then_some(&json!({"effort":"high"}))
            );
            for field in ["cache_control", "eager_input_streaming", "strict"] {
                assert!(body["tools"][0].get(field).is_none());
            }
            if index > 0 {
                replay_anthropic(body)?;
            }
            if index == 2 {
                assert!(body["messages"].to_string().contains("answer β"));
            }
        } else {
            assert_eq!(body["reasoning_effort"], "high");
            assert_eq!(body["tools"][0]["function"]["strict"], false);
            if index > 0 {
                replay(body);
                assert!(body["messages"].to_string().contains("tool α\\n"));
            }
        }
    }
    println!(
        "PASS fireworks/{model}: {anthropic:?} Anthropic, 3 native requests, 2 actual Echo executions, image adaptation and bound later-user history"
    );
    Ok(key)
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    let mut keys = std::collections::BTreeSet::new();
    for (models, anthropic) in [(ANTHROPIC, true), (CHAT, false)] {
        for model in models {
            assert!(keys.insert(scenario(root, mind, model, anthropic).await?));
        }
    }
    assert_eq!(keys.len(), 27);
    println!("PASS Fireworks: 27 models, 81 loopback requests, 54 actual Echo executions");
    Ok(())
}
