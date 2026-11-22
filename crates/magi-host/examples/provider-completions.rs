#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::session::Session;
use magi_host::turn::{Backend, run};
use magi_model::scratch::Scratch;
use magi_proto::{Entry, MessageId, SessionId, StopReason};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const A: &str = r#"{"a":[null,[],{}]}"#;
const B: &str = r#"{"b":18446744073709551615}"#;
const WAIT: Duration = Duration::from_secs(20);

#[path = "provider-completions/tool_history.rs"]
mod tool_history;

#[path = "provider-completions/moonshot.rs"]
mod moonshot;

#[path = "provider-completions/baseten.rs"]
mod baseten;

#[path = "provider-completions/fireworks.rs"]
mod fireworks;

#[path = "provider-completions/qwen.rs"]
mod qwen;

#[path = "provider-completions/together.rs"]
mod together;

#[path = "provider-completions/huggingface.rs"]
mod huggingface;

#[path = "provider-completions/ant_ling.rs"]
mod ant_ling;

#[path = "provider-completions/google_replay.rs"]
mod google_replay;

#[path = "provider-completions/google_thinking.rs"]
mod google_thinking;

struct Echo(Rc<RefCell<Vec<Value>>>);

impl magi_tools::Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Record and echo arguments"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object"})
    }
    fn run(
        &self,
        args: &Value,
        _: &dyn magi_tools::Ops,
        _: &dyn magi_tools::Cancel,
    ) -> magi_tools::Output {
        self.0.borrow_mut().push(args.clone());
        magi_tools::Output::ok(args.to_string())
    }
}

fn chunk(calls: Value) -> Value {
    json!({"choices":[{"index":0,"delta":{"tool_calls":calls}}]})
}

fn call(index: usize, id: &str, args: &str) -> Value {
    json!({"index":index,"id":id,"type":"function","function":{"name":"echo","arguments":args}})
}

fn usage() -> Value {
    json!({"choices":[],"usage":{"prompt_tokens":90,"completion_tokens":12,"prompt_tokens_details":{"cached_tokens":30}}})
}

fn stream(script: Vec<Value>, done: bool) -> String {
    let mut body: String = script.iter().map(|v| format!("data: {v}\n\n")).collect();
    if done {
        body.push_str("data: [DONE]\n\n");
    }
    body
}

fn tool_stream(reason: &str) -> String {
    stream(
        vec![
            chunk(json!([call(0, "a", "{\"a\":"), call(1, "b", "{\"b\":")])),
            chunk(json!([
                {"index":1,"function":{"arguments":"18446744073709551615}"}},
                {"index":0,"function":{"arguments":"[null,[],{}]}"}}
            ])),
            json!({"choices":[{"delta":{},"finish_reason":reason}]}),
            usage(),
        ],
        true,
    )
}

fn text_stream() -> String {
    stream(
        vec![
            json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}),
            usage(),
        ],
        true,
    )
}

fn request(socket: &mut TcpStream, path: &str, client: Option<&str>) -> Result<(String, Value)> {
    request_at(
        socket,
        &format!("{path}/chat/completions"),
        ("authorization", "bearer private-sentinel"),
        client,
    )
}

fn request_at(
    socket: &mut TcpStream,
    path: &str,
    auth: (&str, &str),
    client: Option<&str>,
) -> Result<(String, Value)> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(socket);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        if reader.by_ref().take(8193).read_line(&mut line)? == 0 || line.len() > 8192 {
            return Err("invalid request header".into());
        }
        headers.push_str(&line);
        if headers.len() > 32768 {
            return Err("request headers exceeded bound".into());
        }
        if line == "\r\n" {
            break;
        }
    }
    let headers = headers.to_ascii_lowercase();
    assert!(headers.starts_with(&format!("post {path} ")));
    assert!(headers.contains(&format!("{}: {}\r\n", auth.0, auth.1)));
    assert_eq!(headers.matches(&format!("{}:", auth.0)).count(), 1);
    if let Some(client) = client {
        assert!(headers.contains(&format!("user-agent: {client}\r\n")));
    }
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .ok_or("missing length")?
        .trim()
        .parse()?;
    if length > 1024 * 1024 {
        return Err("request body exceeded bound".into());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok((headers, serde_json::from_slice(&body)?))
}

fn serve(listener: TcpListener, scripts: Vec<String>) -> Result<Vec<Value>> {
    serve_at(listener, scripts, "/v1", None)
}

fn serve_at(
    listener: TcpListener,
    scripts: Vec<String>,
    path: &str,
    client: Option<&str>,
) -> Result<Vec<Value>> {
    Ok(serve_captured(listener, scripts, path, client)?
        .into_iter()
        .map(|(_, body)| body)
        .collect())
}

fn serve_captured(
    listener: TcpListener,
    scripts: Vec<String>,
    path: &str,
    client: Option<&str>,
) -> Result<Vec<(String, Value)>> {
    let mut heard = Vec::new();
    for body in scripts {
        let deadline = Instant::now() + WAIT;
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err("expected loopback request did not arrive".into());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        };
        heard.push(request(&mut socket, path, client)?);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        socket.flush()?;
    }
    Ok(heard)
}

fn backend(root: &Path, mind: &str, url: &str, zai: bool) -> Result<Backend> {
    let compat = if zai {
        "compat={thinking_format=\"zai\",zai_tool_stream=true,supports_reasoning_effort=true},"
    } else {
        ""
    };
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"melchior.provider("fixture", {{name="Fixture",api="openai-completions",base_url={url:?},
            auth={{kind="api-key",vars={{"FIXTURE_KEY"}}}},
            {compat}
            models={{{{id="m",name="Model",reasoning={zai},context_window=8192,max_tokens=128}}}}}})"#
        ),
    )?;
    Ok(Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        tooling: Default::default(),
        cwd: root.to_path_buf(),
        model: "fixture/m".into(),
        mind: mind.into(),
        wants: magi_proto::ask::Wants::default(),
        context_window: Some(8192),
        max_output: Some(128),
        image_resize: None,
        session_identity: false,
        transcript: None,
        system: None,
        confine: false,
        isolate: false,
        grants: Vec::new(),
        environ: Default::default(),
        helpers: Default::default(),
        deciders: Vec::new(),
    })
}

fn user(id: &str) -> Entry {
    Entry::User {
        id: MessageId::new(id),
        text: format!("Use echo twice: {id}"),
        images: Vec::new(),
        aside: String::new(),
    }
}

fn replay(body: &Value) {
    let messages = body["messages"].as_array().expect("messages");
    let assistant = messages
        .iter()
        .find(|m| m["tool_calls"].is_array())
        .expect("calls");
    assert_eq!(assistant["tool_calls"].as_array().expect("calls").len(), 2);
    for (index, (id, args)) in [("a", A), ("b", B)].iter().enumerate() {
        let call = &assistant["tool_calls"][index];
        assert_eq!(call["id"], *id);
        assert_eq!(call["function"]["name"], "echo");
        assert_eq!(call["function"]["arguments"], *args);
        let result = messages
            .iter()
            .find(|m| m["tool_call_id"] == *id)
            .expect("tool result");
        assert_eq!(result["role"], "tool");
        assert_eq!(result["content"], *args);
    }
}

async fn scenario(root: &Path, mind: &str, reason: &str, retry: bool, zai: bool) -> Result<()> {
    let success = reason == "tool_calls";
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1", listener.local_addr()?);
    let backend = backend(root, mind, &url, zai)?;
    let thinking = |text: &str, body: String| {
        if zai {
            format!(
                "data: {}\n\n{body}",
                json!({"choices":[{"delta":{"reasoning_content":text}}]})
            )
        } else {
            body
        }
    };
    let mut scripts = Vec::new();
    if retry {
        scripts.push(stream(
            vec![
                chunk(json!([
                    call(0, "abandoned", "{"),
                    call(1, "buffered", "{}")
                ])),
                usage(),
            ],
            false,
        ));
    }
    scripts.push(thinking("zai tool α\n", tool_stream(reason)));
    if success {
        scripts.extend([
            thinking("zai answer β", text_stream()),
            thinking("zai later γ", text_stream()),
        ]);
    }
    let worker = std::thread::spawn(move || serve(listener, scripts));
    let mut session = Session::recorded(SessionId::new("completions"), Vec::new());
    session.commit(user("first"))?;
    let session = tokio::sync::Mutex::new(session);
    let invoked = Rc::new(RefCell::new(Vec::new()));
    let mut registry = magi_tools::Registry::new();
    registry.register(Box::new(Echo(Rc::clone(&invoked))));
    let ops = magi_tools::ops::Real::new(root.to_path_buf());
    let scribe = Arc::new(tokio::sync::Mutex::new(None));
    tokio::time::timeout(WAIT, run(&session, &backend, &registry, &ops, &scribe)).await??;
    {
        let mut held = session.lock().await;
        let assistants: Vec<_> = held
            .entries()
            .iter()
            .filter_map(|e| match e {
                Entry::Assistant {
                    stop_reason, usage, ..
                } => Some((*stop_reason, *usage)),
                _ => None,
            })
            .collect();
        let expected = match reason {
            "tool_calls" => StopReason::ToolUse,
            "length" | "repetition_truncation" => StopReason::Length,
            _ => StopReason::Error,
        };
        assert_eq!(assistants[0].0, Some(expected));
        assert_eq!(assistants[0].1.output, if retry { 24 } else { 12 });
        let calls: Vec<_> = held
            .entries()
            .iter()
            .filter_map(|e| match e {
                Entry::Tool {
                    id, args, result, ..
                } => Some((id.to_string(), args, result)),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls.len(),
            if expected == StopReason::Error { 0 } else { 2 }
        );
        for ((id, args, result), (wanted_id, wanted_args)) in calls.iter().zip([("a", A), ("b", B)])
        {
            assert_eq!(id, wanted_id);
            assert_eq!(*args, wanted_args);
            assert_eq!(
                result.as_ref().expect("transcript result").is_error,
                !success
            );
        }
        assert_eq!(
            held.take_pending()
                .iter()
                .filter(|(_, e)| matches!(e, Entry::Tool { .. }))
                .count(),
            if expected == StopReason::Length { 2 } else { 0 },
            "a further provider round flushes pending entries when no scribe is installed"
        );
        if success {
            held.commit(user("later"))?;
        }
    }
    if success {
        tokio::time::timeout(WAIT, run(&session, &backend, &registry, &ops, &scribe)).await??;
        assert_eq!(
            *invoked.borrow(),
            vec![
                serde_json::from_str::<Value>(A)?,
                serde_json::from_str::<Value>(B)?
            ]
        );
    } else {
        assert!(invoked.borrow().is_empty(), "interrupted calls executed");
    }
    let heard = worker.join().map_err(|_| "loopback server panicked")??;
    let offset = usize::from(retry);
    if retry {
        assert_eq!(heard[0], heard[1], "retry changed request");
    }
    if success {
        replay(&heard[offset + 1]);
        replay(&heard[offset + 2]);
        if zai {
            for body in &heard {
                assert_eq!(body["tool_stream"], true);
                assert_eq!(
                    body["thinking"],
                    json!({"type":"enabled","clear_thinking":false})
                );
                assert!(body.get("reasoning_effort").is_none());
            }
            for body in [&heard[offset + 1], &heard[offset + 2]] {
                let assistant = body["messages"]
                    .as_array()
                    .expect("history")
                    .iter()
                    .find(|m| m["tool_calls"].is_array())
                    .expect("assistant tool calls");
                assert_eq!(assistant["reasoning_content"], "zai tool α\n");
            }
            assert!(
                heard[offset + 2]["messages"]
                    .as_array()
                    .expect("history")
                    .iter()
                    .any(|m| m["reasoning_content"] == "zai answer β")
            );
        }
        assert!(
            heard[offset + 2]["messages"]
                .as_array()
                .expect("messages")
                .iter()
                .any(|m| m["role"] == "user" && m["content"] == "Use echo twice: later")
        );
    }
    println!(
        "PASS {reason} retry={retry} zai={zai}: {} loopback requests, {} actual tool executions",
        heard.len(),
        invoked.borrow().len()
    );
    Ok(())
}

async fn instructions(root: &Path, mind: &str, positional: bool) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1", listener.local_addr()?);
    let declaration = json!({"name":"Fixture","api":"openai-completions","base_url":url,
        "auth":{"kind":"api-key","vars":["FIXTURE_KEY"]},
        "compat":{"supports_mid_convo_system_messages":positional},
        "models":[{"id":"m","name":"M","context_window":8192,"max_tokens":128}]});
    let encoded = serde_json::to_string(&declaration.to_string())?;
    std::fs::write(
        root.join("providers.lua"),
        format!("melchior.provider(\"fixture\", melchior.json.decode_value({encoded}))"),
    )?;
    let worker = std::thread::spawn(move || serve(listener, vec![text_stream()]));
    let context = magi_model::Context {
        system: Some("base".into()),
        messages: vec![
            magi_model::Message::user("first"),
            magi_model::Message::assistant("answer"),
            magi_model::Message::system("update α"),
            magi_model::Message::user("next"),
        ],
        ..magi_model::Context::default()
    };
    let saved = context.clone();
    let mut deltas = Vec::new();
    let mut retries = 0;
    tokio::time::timeout(
        WAIT,
        magi_host::broker::ask_through(
            mind,
            "fixture/m",
            &context,
            &magi_proto::ask::Wants::default(),
            |delta| deltas.push(delta),
            |_| retries += 1,
        ),
    )
    .await?
    .map_err(|error| error.message)?;
    assert_eq!(retries, 0);
    assert!(
        deltas
            .iter()
            .any(|d| matches!(d, magi_model::Delta::Stop(StopReason::EndTurn)))
    );
    assert_eq!(context, saved);
    let heard = worker.join().map_err(|_| "instruction server panicked")??;
    let messages = &heard[0]["messages"];
    if positional {
        assert_eq!(
            messages,
            &json!([
                {"role":"system","content":"base"},{"role":"user","content":"first"},
                {"role":"assistant","content":"answer"},{"role":"system","content":"update α"},
                {"role":"user","content":"next"}
            ])
        );
    } else {
        assert_eq!(
            messages,
            &json!([
                {"role":"system","content":"base\n\nupdate α"},{"role":"user","content":"first"},
                {"role":"assistant","content":"answer"},{"role":"user","content":"next"}
            ])
        );
    }
    println!(
        "PASS instructions positional={positional}: actual Magi broker and selected Melchior, one loopback request"
    );
    Ok(())
}

async fn worker(root: &Path, mind: &str) -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--google-thinking") {
        return google_thinking::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--google-replay") {
        return google_replay::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--moonshot") {
        return moonshot::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--baseten") {
        return baseten::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--fireworks") {
        return fireworks::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--qwen") {
        return qwen::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--together") {
        return together::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--huggingface") {
        return huggingface::run_all(root, mind).await;
    }
    if std::env::args().nth(1).as_deref() == Some("--ant-ling") {
        return ant_ling::run_all(root, mind).await;
    }
    scenario(root, mind, "tool_calls", false, false).await?;
    scenario(root, mind, "tool_calls", true, false).await?;
    scenario(root, mind, "tool_calls", false, true).await?;
    scenario(root, mind, "tool_calls", true, true).await?;
    for reason in [
        "length",
        "repetition_truncation",
        "content_filter",
        "future_finish",
    ] {
        scenario(root, mind, reason, false, false).await?;
    }
    instructions(root, mind, false).await?;
    instructions(root, mind, true).await?;
    for native in [false, true] {
        for strict in [false, true] {
            tool_history::scenario(root, mind, native, strict).await?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some(magi_host::images::WORKER_ARG) {
        if std::env::args_os().count() != 2 {
            return Err("image worker takes only a bounded stdin request".into());
        }
        return magi_host::images::worker().map_err(Into::into);
    }
    let mind = std::fs::canonicalize(std::env::var("MELCHIOR_TEST_BINARY")?)?;
    if let Some(root) = std::env::var_os("MAGI_PROVIDER_FIXTURE_ROOT") {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(worker(
                Path::new(&root),
                mind.to_str().ok_or("non-UTF8 binary")?,
            ));
    }
    let dir = Scratch::new("magi-provider", "completions");
    let mut command = std::process::Command::new(std::env::current_exe()?);
    if let Some(
        mode @ ("--moonshot" | "--baseten" | "--fireworks" | "--qwen" | "--together"
        | "--huggingface" | "--ant-ling"),
    ) = std::env::args().nth(1).as_deref()
    {
        let source = std::fs::canonicalize(std::env::var("MELCHIOR_PROVIDER_SOURCE")?)?;
        let bytes = std::fs::read(&source)?;
        let digest = Sha256::digest(&bytes);
        println!(
            "Selected provider source: {} sha256={digest:x}",
            source.display()
        );
        std::fs::write(dir.join("shipped-providers.lua"), bytes)?;
        if mode == "--huggingface" {
            let reference = std::fs::canonicalize(std::env::var("MELCHIOR_CATALOG_REFERENCE")?)?;
            let bytes = std::fs::read(&reference)?;
            let digest = Sha256::digest(&bytes);
            assert_eq!(
                format!("{digest:x}"),
                "c03ec1c80856b805dd67253026376c8931683c014809a0e4b86e21c1a3978968"
            );
            println!(
                "Selected Hugging Face reference: {} sha256={digest:x}",
                reference.display()
            );
            std::fs::write(dir.join("huggingface.json"), bytes)?;
        }
        command.arg(mode);
    }
    if let Some(mode @ ("--google-replay" | "--google-thinking")) =
        std::env::args().nth(1).as_deref()
    {
        command.arg(mode);
    }
    command
        .env_clear()
        .current_dir(&dir)
        .env("MELCHIOR_TEST_BINARY", &mind)
        .env("MAGI_PROVIDER_FIXTURE_ROOT", dir.as_os_str())
        .env("HOME", dir.as_os_str())
        .env("MELCHIOR_CONFIG", dir.as_os_str())
        .env("FIXTURE_KEY", "private-sentinel");
    command
        .env("MOONSHOT_API_KEY", "private-sentinel")
        .env("BASETEN_API_KEY", "private-sentinel")
        .env("FIREWORKS_API_KEY", "private-sentinel")
        .env("TOGETHER_API_KEY", "private-sentinel")
        .env("HF_TOKEN", "private-sentinel")
        .env("ANT_LING_API_KEY", "private-sentinel")
        .env("KIMI_API_KEY", "private-sentinel");
    for key in qwen::KEYS {
        command.env(key, "private-sentinel");
    }
    for (name, path) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
    ] {
        command.env(name, dir.private(path));
    }
    let digest = Sha256::digest(std::fs::read(&mind)?);
    println!("Selected Melchior: {} sha256={digest:x}", mind.display());
    if !command.status()?.success() {
        return Err("native host provider fixture failed".into());
    }
    assert_eq!(
        Sha256::digest(std::fs::read(&mind)?),
        digest,
        "selected binary changed"
    );
    Ok(())
}
