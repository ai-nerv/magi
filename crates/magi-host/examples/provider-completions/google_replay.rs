use super::*;

const MODEL: &str = "gemini-3-flash-preview";
const OTHER: &str = "gemini-3-pro-preview";
const PREFIX: &str = "melchior.google.v1:";

fn parts() -> Value {
    json!([
        {"text":"thought α", "thought":true,"thoughtSignature":"dGhvdWdodA=="},
        {"text":"before", "thoughtSignature":"YmVmb3Jl"},
        {"text":"", "thoughtSignature":"ZW1wdHk="},
        {"functionCall":{"id":"a","name":"echo","args":serde_json::from_str::<Value>(A).expect("A")},"thoughtSignature":"Y2FsbGE="},
        {"text":"between", "thought":true,"thoughtSignature":"YmV0d2Vlbg=="},
        {"functionCall":{"id":"b","name":"echo","args":serde_json::from_str::<Value>(B).expect("B")},"thoughtSignature":"Y2FsbGI="}
    ])
}

fn response(parts: &Value, reason: &str) -> String {
    let mut body = String::new();
    for part in parts.as_array().expect("parts") {
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"candidates":[{"content":{"parts":[part]}}]})
        ));
    }
    body.push_str(&format!("data: {}\n\n", json!({"candidates":[{"finishReason":reason}],
        "usageMetadata":{"promptTokenCount":90,"cachedContentTokenCount":30,"candidatesTokenCount":12,"thoughtsTokenCount":7}})));
    body
}

fn route(api: &str, model: &str) -> String {
    let prefix = if api == "google-vertex" {
        "/v1/publishers/google"
    } else {
        "/v1"
    };
    format!("{prefix}/models/{model}:streamGenerateContent?alt=sse").to_ascii_lowercase()
}

fn serve_google(
    listener: TcpListener,
    api: &'static str,
    scripts: Vec<String>,
    switched: bool,
    retry: bool,
) -> Result<Vec<Value>> {
    let mut heard = Vec::new();
    for (index, body) in scripts.into_iter().enumerate() {
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
        let round = index.saturating_sub(usize::from(retry));
        let key = if switched && round == 4 {
            "other-private-sentinel"
        } else {
            "private-sentinel"
        };
        let bearer = format!("bearer {key}");
        let auth = if api == "google-vertex" {
            ("authorization", bearer.as_str())
        } else {
            ("x-goog-api-key", key)
        };
        let model = if switched && round == 5 { OTHER } else { MODEL };
        heard.push(request_at(&mut socket, &route(api, model), auth, None)?.1);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        socket.flush()?;
    }
    Ok(heard)
}

fn configured(root: &Path, mind: &str, url: &str, api: &str) -> Result<Backend> {
    let mut backend = super::backend(root, mind, url, false)?;
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"
        for _, id in ipairs({{"openrouter","ollama"}}) do
          melchior.provider(id, {{name=id,api="openai-completions",auth={{kind="none"}}}})
        end
        melchior.provider("fixture", {{name="Fixture",api={api:?},base_url={url:?},
          auth={{kind="api-key",vars={{"FIXTURE_KEY"}}}},models={{
            {{id={MODEL:?},name="Model",reasoning=true,context_window=200000,max_tokens=4096,cost={{input=2,output=3,cache_read=0.5}}}},
            {{id={OTHER:?},name="Other",reasoning=true,context_window=200000,max_tokens=4096,cost={{input=2,output=3,cache_read=0.5}}}}
          }} }})
    "#
        ),
    )?;
    backend.model = format!("fixture/{MODEL}");
    backend.context_window = Some(200000);
    backend.max_output = Some(4096);
    backend.wants.max_tokens = Some(128);
    Ok(backend)
}

type Rows = Vec<(magi_proto::Cursor, Entry)>;

fn rows(session: &Session) -> Rows {
    session
        .entries()
        .iter()
        .enumerate()
        .map(|(index, entry)| (session.cursor_at(index).expect("cursor"), entry.clone()))
        .collect()
}

fn restore(root: &Path, rows: &Rows, cbor: bool) -> Result<Session> {
    let path = root.join(if cbor {
        "google-history.cbor"
    } else {
        "google-history.json"
    });
    let decoded: Rows = if cbor {
        let mut bytes = Vec::new();
        ciborium::into_writer(rows, &mut bytes)?;
        std::fs::write(&path, bytes)?;
        ciborium::from_reader(std::fs::File::open(path)?)?
    } else {
        std::fs::write(&path, serde_json::to_vec(rows)?)?;
        serde_json::from_slice(&std::fs::read(path)?)?
    };
    assert_eq!(decoded, *rows);
    Ok(Session::restored(SessionId::new("google"), decoded)?)
}

fn model_parts(body: &Value) -> Vec<&Value> {
    body["contents"]
        .as_array()
        .expect("contents")
        .iter()
        .filter(|message| message["role"] == "model")
        .map(|message| &message["parts"])
        .collect()
}

async fn scenario(
    root: &Path,
    mind: &str,
    api: &'static str,
    reason: &str,
    retry: bool,
) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let mut backend = configured(
        root,
        mind,
        &format!("http://{}/v1", listener.local_addr()?),
        api,
    )?;
    let success = reason == "STOP";
    let answer = json!([{"text":"done","thoughtSignature":"ZG9uZQ=="},{"text":"","thoughtSignature":"dGFpbA=="}]);
    let empty = json!([{"text":"","thoughtSignature":"b25seQ=="}]);
    let mut scripts = vec![response(&parts(), reason)];
    if retry {
        scripts.insert(0, format!("data: {}\n\n", json!({"candidates":[{"content":{"parts":[
            {"text":"abandoned","thought":true,"thoughtSignature":"YWJhbmRvbmVk"},
            {"functionCall":{"id":"abandoned","name":"echo","args":{"abandoned":true}},"thoughtSignature":"YWJhbmRvbmVk"}
        ]}}],"usageMetadata":{"promptTokenCount":90,"cachedContentTokenCount":30,"candidatesTokenCount":5,"thoughtsTokenCount":3}})));
    }
    if success {
        scripts.extend([
            response(&answer, "STOP"),
            response(&empty, "STOP"),
            response(&json!([{"text":"final"}]), "STOP"),
            response(&json!([{"text":"new account"}]), "STOP"),
            response(&json!([{"text":"new model"}]), "STOP"),
        ]);
    }
    let worker = std::thread::spawn(move || serve_google(listener, api, scripts, success, retry));
    let mut initial = Session::recorded(SessionId::new("google"), Vec::new());
    initial.commit(user("first"))?;
    let session = tokio::sync::Mutex::new(initial);
    let invoked = Rc::new(RefCell::new(Vec::new()));
    let mut registry = magi_tools::Registry::new();
    registry.register(Box::new(Echo(Rc::clone(&invoked))));
    let ops = magi_tools::ops::Real::new(root.to_path_buf());
    let scribe = Arc::new(tokio::sync::Mutex::new(None));
    tokio::time::timeout(WAIT, run(&session, &backend, &registry, &ops, &scribe)).await??;
    let saved = rows(&*session.lock().await);
    let usage = saved
        .iter()
        .find_map(|(_, entry)| match entry {
            Entry::Assistant { usage, .. } => Some(usage),
            _ => None,
        })
        .ok_or("Google assistant usage missing")?;
    assert_eq!(usage.output, if retry { 27 } else { 19 });
    assert_eq!(usage.input, if retry { 120 } else { 60 });
    assert_eq!(usage.cache_read, if retry { 60 } else { 30 });
    assert_eq!(usage.cost_micros, 0);
    assert_eq!(usage.estimated_micros, Some(if retry { 351 } else { 192 }));
    if success {
        assert_eq!(
            *invoked.borrow(),
            vec![
                serde_json::from_str::<Value>(A)?,
                serde_json::from_str::<Value>(B)?
            ]
        );
        let serialized = serde_json::to_string(&saved)?;
        assert!(serialized.contains(PREFIX));
        assert!(!serialized.contains("private-sentinel"));
        assert!(!serialized.contains("abandoned"));
        assert!(saved.iter().any(|(_, entry)| matches!(entry,
            Entry::Assistant { stop_reason: Some(StopReason::ToolUse), usage, .. }
            if usage.output == if retry { 27 } else { 19 })));
        let mut restored = restore(root, &saved, false)?;
        restored.commit(user("later-json"))?;
        let restored = tokio::sync::Mutex::new(restored);
        tokio::time::timeout(WAIT, run(&restored, &backend, &registry, &ops, &scribe)).await??;
        let after_empty = rows(&*restored.lock().await);
        let mut restored = restore(root, &after_empty, true)?;
        restored.commit(user("later-cbor"))?;
        let restored = tokio::sync::Mutex::new(restored);
        tokio::time::timeout(WAIT, run(&restored, &backend, &registry, &ops, &scribe)).await??;
        for changed_model in [false, true] {
            let mut switched = restore(root, &saved, changed_model)?;
            switched.commit(user("switched"))?;
            if changed_model {
                backend.environ.clear();
                backend.model = format!("fixture/{OTHER}");
            } else {
                backend
                    .environ
                    .insert("FIXTURE_KEY".into(), "other-private-sentinel".into());
            }
            let switched = tokio::sync::Mutex::new(switched);
            tokio::time::timeout(WAIT, run(&switched, &backend, &registry, &ops, &scribe))
                .await??;
        }
    } else {
        assert!(invoked.borrow().is_empty());
        assert!(!serde_json::to_string(&saved)?.contains(PREFIX));
        assert!(saved.iter().any(
            |(_, entry)| matches!(entry, Entry::Assistant { stop_reason: Some(stop), .. }
            if *stop == if reason == "MAX_TOKENS" { StopReason::Length } else { StopReason::Error })
        ));
    }
    let heard = worker.join().map_err(|_| "Google server panicked")??;
    assert_eq!(
        heard.len(),
        if success { 6 + usize::from(retry) } else { 1 }
    );
    let heard = &heard[usize::from(retry)..];
    if success {
        for body in &heard[1..4] {
            assert_eq!(model_parts(body)[0], &parts());
        }
        assert_eq!(model_parts(&heard[2])[1], &answer);
        assert_eq!(model_parts(&heard[3])[2], &empty);
        for body in &heard[4..] {
            assert!(!body.to_string().contains("thoughtSignature"));
            assert!(body.to_string().contains("functionCall"));
        }
    }
    for body in heard {
        assert!(body.get("__scratch").is_none());
        assert!(body.get("__budget").is_none());
        assert!(!body.to_string().contains(PREFIX));
    }
    println!(
        "PASS {api}/{reason}/retry={retry}: actual host tools, signed parts, thought usage/pricing, restored history and origin isolation"
    );
    Ok(())
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    for api in ["google-generative-ai", "google-vertex"] {
        for reason in ["STOP", "MAX_TOKENS", "SAFETY"] {
            scenario(root, mind, api, reason, false).await?;
        }
        scenario(root, mind, api, "STOP", true).await?;
    }
    println!("PASS Google replay: 2 APIs, 8 scenarios, 30 loopback requests, 8 Echo executions");
    Ok(())
}
