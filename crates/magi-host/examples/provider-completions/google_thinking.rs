use super::*;
use magi_model::ThinkingLevel;

struct Case {
    id: &'static str,
    menu: &'static [&'static str],
    values: [&'static str; 8],
}

const CASES: &[Case] = &[
    Case {
        id: "gemini-2.5-pro",
        menu: &["minimal", "low", "medium", "high"],
        values: [
            "128", "128", "128", "2048", "8192", "32768", "32768", "32768",
        ],
    },
    Case {
        id: "gemini-2.5-flash-lite",
        menu: &["off", "minimal", "low", "medium", "high"],
        values: ["0", "0", "512", "2048", "8192", "24576", "24576", "24576"],
    },
    Case {
        id: "gemini-3-pro",
        menu: &["low", "high"],
        values: ["LOW", "LOW", "LOW", "LOW", "HIGH", "HIGH", "HIGH", "HIGH"],
    },
    Case {
        id: "gemini-3-flash-preview",
        menu: &["minimal", "low", "medium", "high"],
        values: [
            "MINIMAL", "MINIMAL", "MINIMAL", "LOW", "MEDIUM", "HIGH", "HIGH", "HIGH",
        ],
    },
    Case {
        id: "gemini-4-neon",
        menu: &["low", "medium", "high", "xhigh"],
        values: [
            "LOW", "LOW", "LOW", "LOW", "MEDIUM", "HIGH", "XHIGH", "XHIGH",
        ],
    },
    Case {
        id: "gemma-4-27b-it",
        menu: &["minimal", "high"],
        values: [
            "MINIMAL", "MINIMAL", "MINIMAL", "HIGH", "HIGH", "HIGH", "HIGH", "HIGH",
        ],
    },
    Case {
        id: "gemini-robotics-er-2",
        menu: &["minimal", "low", "medium", "high"],
        values: [
            "MINIMAL", "MINIMAL", "MINIMAL", "LOW", "MEDIUM", "HIGH", "HIGH", "HIGH",
        ],
    },
    Case {
        id: "plain",
        menu: &[],
        values: [""; 8],
    },
    Case {
        id: "restricted",
        menu: &[],
        values: [""; 8],
    },
];

fn configure(root: &Path, api: &str, url: &str) -> Result<()> {
    let models: Vec<_> = CASES
        .iter()
        .map(|case| {
            let mut model = json!({"id":case.id,"name":case.id,"reasoning":case.id != "plain",
            "context_window":100000,"max_tokens":65536});
            if case.id.starts_with("gemma-") {
                model["thinking"] =
                    json!({"off":null,"minimal":"MINIMAL","low":null,"medium":null,"high":"HIGH"});
            }
            if case.menu.is_empty() {
                model["capabilities"] = json!({"thinking_levels":[]});
            }
            model
        })
        .collect();
    let provider = json!({"name":"Fixture","api":api,"base_url":url,
        "auth":{"kind":"api-key","vars":["FIXTURE_KEY"]},"models":models});
    let quoted = serde_json::to_string(&provider.to_string())?;
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"
        for _, id in ipairs({{"openrouter","ollama"}}) do
          melchior.provider(id, {{name=id,api="openai-completions",auth={{kind="none"}}}})
        end
        melchior.provider("fixture", melchior.json.decode_value({quoted}))
    "#
        ),
    )?;
    Ok(())
}

fn serve_controls(listener: TcpListener, api: &'static str) -> Result<usize> {
    let mut requests = 0;
    for case in CASES {
        for (index, level) in std::iter::once(None)
            .chain(ThinkingLevel::ALL.map(Some))
            .enumerate()
        {
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
            let prefix = if api == "google-vertex" {
                "/v1/publishers/google"
            } else {
                "/v1"
            };
            let path = format!("{prefix}/models/{}:streamGenerateContent?alt=sse", case.id)
                .to_ascii_lowercase();
            let auth = if api == "google-vertex" {
                ("authorization", "bearer private-sentinel")
            } else {
                ("x-goog-api-key", "private-sentinel")
            };
            let (_, body) = request_at(&mut socket, &path, auth, None)?;
            let mut expected = Value::Null;
            if !case.menu.is_empty() {
                expected = match case.values[index].parse::<i32>() {
                    Ok(budget) => json!({"thinkingBudget":budget}),
                    Err(_) => json!({"thinkingLevel":case.values[index]}),
                };
                if level.is_some_and(|value| value != ThinkingLevel::Off) {
                    expected["includeThoughts"] = json!(true);
                }
            }
            assert_eq!(
                body["generationConfig"]["thinkingConfig"], expected,
                "{api}/{} {level:?}",
                case.id
            );
            assert_eq!(body["generationConfig"]["maxOutputTokens"], 128);
            for private in [
                "__google_control",
                "__budget",
                "__scratch",
                "__credential_binding",
            ] {
                assert!(body.get(private).is_none());
            }
            let answer = format!(
                "data: {}\n\n",
                json!({"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":2,"cachedContentTokenCount":0,"candidatesTokenCount":1,"thoughtsTokenCount":1}})
            );
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            )?;
            socket.flush()?;
            requests += 1;
        }
    }
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    Ok(requests)
}

pub(super) async fn run_all(root: &Path, mind: &str) -> Result<()> {
    let mut requests = 0;
    for api in ["google-generative-ai", "google-vertex"] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        configure(root, api, &format!("http://{}/v1", listener.local_addr()?))?;
        let mut catalog = magi_host::catalog::Catalog::empty();
        catalog.cwd = root.to_path_buf();
        catalog.mind = mind.into();
        catalog.cards = magi_host::broker::cards(mind).await;
        assert_eq!(
            catalog
                .cards
                .iter()
                .filter(|card| card.provider == "fixture")
                .count(),
            CASES.len()
        );
        let server = std::thread::spawn(move || serve_controls(listener, api));
        for case in CASES {
            let id = format!("fixture/{}", case.id);
            let card = catalog
                .cards
                .iter()
                .find(|card| card.id == id)
                .ok_or("missing card")?;
            assert!(card.ready);
            assert_eq!(card.api, api);
            assert_eq!(card.reasons, case.id != "plain");
            assert_eq!(
                card.capabilities
                    .as_ref()
                    .ok_or("missing capabilities")?
                    .thinking_levels,
                Some(case.menu.iter().map(|level| (*level).to_owned()).collect())
            );
            let mut backend = catalog.backend(&id).ok_or("missing backend")?;
            backend.confine = false;
            backend.isolate = false;
            backend.wants.max_tokens = Some(128);
            for level in std::iter::once(None).chain(ThinkingLevel::ALL.map(Some)) {
                backend.wants.thinking = level;
                let before = serde_json::to_value(&backend.wants)?;
                let mut session = Session::recorded(SessionId::new("thinking"), Vec::new());
                session.commit(user("first"))?;
                let session = tokio::sync::Mutex::new(session);
                let registry = magi_tools::Registry::new();
                let ops = magi_tools::ops::Real::new(root.to_path_buf());
                let scribe = Arc::new(tokio::sync::Mutex::new(None));
                tokio::time::timeout(WAIT, run(&session, &backend, &registry, &ops, &scribe))
                    .await??;
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
                assert_eq!(assistants.len(), 1);
                assert_eq!(assistants[0].0, Some(StopReason::EndTurn));
                assert_eq!((assistants[0].1.input, assistants[0].1.output), (2, 2));
                assert_eq!(serde_json::to_value(&backend.wants)?, before);
            }
        }
        requests += server
            .join()
            .map_err(|_| "Google thinking server panicked")??;
    }
    assert_eq!(requests, 144);
    println!(
        "PASS Google thinking: 2 APIs, 9 models, 144 host requests, exact menus/controls and immutable options"
    );
    Ok(())
}
