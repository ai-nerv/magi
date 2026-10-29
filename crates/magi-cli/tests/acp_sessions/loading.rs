use super::{turns::*, *};
use std::os::unix::ffi::OsStringExt;

fn environment(fixture: &Fixture) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    std::fs::read(format!(
        "/proc/{}/environ",
        fixture.child.id().expect("adapter PID")
    ))
    .expect("fixture environment")
    .split(|byte| *byte == 0)
    .filter_map(|item| {
        let at = item.iter().position(|byte| *byte == b'=')?;
        Some((
            std::ffi::OsString::from_vec(item[..at].to_vec()),
            std::ffi::OsString::from_vec(item[at + 1..].to_vec()),
        ))
    })
    .collect()
}

async fn restart(fixture: Fixture) -> Fixture {
    let env = environment(&fixture);
    let binary = fixture.binary.clone();
    let root = fixture.close().await;
    let mut child = tokio::process::Command::new(&binary)
        .arg("acp")
        .env_clear()
        .envs(env)
        .current_dir(&*root)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("restart adapter");
    let input = child.stdin.take();
    let output = BufReader::new(child.stdout.take().expect("stdout"));
    let mut fixture = Fixture {
        binary,
        child,
        input,
        output,
        root,
    };
    let ready = fixture
        .call(1, "initialize", json!({"protocolVersion":1}))
        .await;
    assert_eq!(ready["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        std::fs::read_link(format!("/proc/{}/exe", fixture.child.id().expect("PID")))
            .expect("adapter executable"),
        fixture.binary
    );
    fixture
}

async fn finish(fixture: &mut Fixture, id: u64) {
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], id);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            break;
        }
    }
}

async fn seeded(fixture: &mut Fixture, name: &str) -> (Value, magi_testkit::Mind) {
    let mind = magi_testkit::Mind::saying(
        name,
        &[
            &magi_testkit::mind::text_line("durable answer"),
            &json!({"event":"spent","usage":charged()}).to_string(),
            &magi_testkit::mind::stop_line(),
        ],
    );
    synthetic(fixture, &mind);
    let id = session(fixture).await;
    prompt(fixture, 3, &id, "durable question").await;
    finish(fixture, 3).await;
    assert_eq!(mind.asked(), 1);
    (id, mind)
}

fn charged() -> magi_proto::Usage {
    magi_proto::Usage {
        input: 30,
        output: 7,
        cache_read: 4,
        cache_write: 2,
        cost_micros: 55,
        estimated_micros: Some(13),
    }
}

async fn recorded_usage(fixture: &Fixture) -> magi_proto::Usage {
    tokio::time::timeout(Duration::from_secs(10), async {
        let socket = std::fs::read_dir(fixture.root.join("r/magi"))
            .expect("runtime")
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with("acp-"))
            .expect("owned host")
            .path()
            .join("host");
        let (read, write) = magi_ipc::connect(&socket)
            .await
            .expect("native snapshot")
            .into_split();
        magi_ipc::FrameWriter::new(write)
            .write(&magi_proto::UiCommand::AttachLatest {
                draws: false,
                key: magi_ipc::read_key(&socket),
            })
            .await
            .expect("attach");
        let magi_proto::HarnessEvent::SessionSnapshot { entries, .. } =
            magi_ipc::FrameReader::new(read)
                .read::<magi_proto::HarnessEvent>()
                .await
                .expect("snapshot")
        else {
            panic!("expected native snapshot");
        };
        entries
            .iter()
            .fold(magi_proto::Usage::default(), |total, entry| match entry {
                magi_proto::Entry::Assistant { usage, .. } => total.plus(*usage),
                _ => total,
            })
    })
    .await
    .expect("native usage deadline")
}

async fn loading(fixture: &mut Fixture, request: u64, session: &Value) {
    fixture
        .send(
            json!({"jsonrpc":"2.0","id":request,"method":"session/load","params":{
                "sessionId":session,"cwd":fixture.root.join("a"),"mcpServers":[]
            }}),
        )
        .await;
}

async fn loaded(fixture: &mut Fixture, request: u64, session: &Value) {
    let mut users = String::new();
    let mut assistants = String::new();
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], request);
            assert!(frame.get("result").is_some(), "{frame}");
            assert!(frame["result"].get("sessionId").is_none());
            assert!(frame["result"]["configOptions"].is_array());
            break;
        }
        assert_eq!(frame["method"], "session/update");
        assert_eq!(frame["params"]["sessionId"], *session);
        let update = &frame["params"]["update"];
        let text = update["content"]["text"].as_str().unwrap_or("");
        match update["sessionUpdate"].as_str() {
            Some("user_message_chunk") => users.push_str(text),
            Some("agent_message_chunk") => assistants.push_str(text),
            _ => {}
        }
    }
    assert_eq!(users, "durable question");
    assert_eq!(assistants, "durable answer");
}

#[tokio::test]
async fn native_acp_load_replays_before_response_and_continues_without_provider_replay() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let (id, mind) = seeded(&mut fixture, "acp-load-replay").await;
    assert_eq!(recorded_usage(&fixture).await, charged());
    let mut fixture = restart(fixture).await;
    let foreign = fixture
        .call(
            4,
            "session/load",
            json!({
                "sessionId":id,"cwd":fixture.root.join("b"),"mcpServers":[]
            }),
        )
        .await;
    assert!(foreign.get("error").is_some(), "{foreign}");
    assert!(fixture.host_pids().is_empty());
    loading(&mut fixture, 2, &id).await;
    loaded(&mut fixture, 2, &id).await;
    assert_eq!(mind.asked(), 1, "history replay must not execute a turn");
    assert_eq!(
        recorded_usage(&fixture).await,
        charged(),
        "replay preserves token/cache/cost totals exactly"
    );
    prompt(&mut fixture, 3, &id, "continued after reload").await;
    finish(&mut fixture, 3).await;
    assert_eq!(mind.asked(), 2);
    assert_eq!(
        recorded_usage(&fixture).await,
        charged().plus(charged()),
        "only the new turn adds usage"
    );
    let resumed = &mind.asks()[1];
    assert!(resumed.contains("durable question") && resumed.contains("durable answer"));
    assert!(resumed.contains("continued after reload"));
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_load_refuses_duplicate_missing_and_foreign_store_without_losing_owner() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let (id, mind) = seeded(&mut fixture, "acp-load-refused").await;
    for (request, key, cwd) in [
        (4, id.clone(), "a"),
        (5, json!("magi-v1-6d697373696e67"), "a"),
        (6, id.clone(), "b"),
    ] {
        let response = fixture
            .call(
                request,
                "session/load",
                json!({
                    "sessionId":key,"cwd":fixture.root.join(cwd),"mcpServers":[]
                }),
            )
            .await;
        assert!(response.get("error").is_some(), "{response}");
        assert_eq!(
            fixture.host_pids().len(),
            1,
            "failed load reaps its own host only"
        );
    }
    prompt(&mut fixture, 7, &id, "owner survives failed loads").await;
    finish(&mut fixture, 7).await;
    assert_eq!(mind.asked(), 2);
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_concurrent_loads_admit_exactly_one_writer() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let (id, mind) = seeded(&mut fixture, "acp-load-concurrent").await;
    let mut fixture = restart(fixture).await;
    loading(&mut fixture, 2, &id).await;
    loading(&mut fixture, 3, &id).await;
    let (mut success, mut refused, mut users, mut assistants) =
        (0, 0, String::new(), String::new());
    while success + refused < 2 {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert!(frame["id"] == 2 || frame["id"] == 3);
            if frame.get("result").is_some() {
                success += 1;
            } else {
                refused += 1;
            }
        } else {
            assert_eq!(frame["method"], "session/update");
            assert_eq!(frame["params"]["sessionId"], id);
            let update = &frame["params"]["update"];
            let text = update["content"]["text"].as_str().unwrap_or("");
            match update["sessionUpdate"].as_str() {
                Some("user_message_chunk") => users.push_str(text),
                Some("agent_message_chunk") => assistants.push_str(text),
                _ => {}
            }
        }
    }
    assert_eq!((success, refused), (1, 1));
    assert_eq!(users, "durable question");
    assert_eq!(assistants, "durable answer");
    assert_eq!(mind.asked(), 1);
    assert_eq!(fixture.host_pids().len(), 1);
    prompt(&mut fixture, 4, &id, "winning writer continues").await;
    finish(&mut fixture, 4).await;
    assert_eq!(mind.asked(), 2);
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_load_cannot_steal_an_ordinary_host_writer() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let (id, mind) = seeded(&mut fixture, "acp-load-ordinary").await;
    let mut fixture = restart(fixture).await;
    let encoded = id
        .as_str()
        .expect("external ID")
        .strip_prefix("magi-v1-")
        .expect("version");
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex text"), 16).expect("hex byte")
        })
        .collect();
    let native = String::from_utf8(bytes).expect("native transcript");
    let socket = fixture.root.join("ordinary.sock");
    let mut ordinary = tokio::process::Command::new(&fixture.binary)
        .args(["--headless", "--resume-run", &native, "--socket"])
        .arg(&socket)
        .env_clear()
        .envs(environment(&fixture))
        .env_remove("MAGI_API_SOCKET")
        .env_remove("MAGI_MEMORY_INSTANCE")
        .env_remove("MAGI_MELCHIOR_SESSION")
        .current_dir(fixture.root.join("a"))
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("ordinary host");
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(ordinary.try_wait().expect("ordinary status").is_none());
            if let Ok(stream) = magi_ipc::connect(&socket).await {
                let (read, write) = stream.into_split();
                magi_ipc::FrameWriter::new(write)
                    .write(&magi_proto::UiCommand::AttachLatest {
                        draws: false,
                        key: magi_ipc::read_key(&socket),
                    })
                    .await
                    .expect("ordinary controller");
                let snapshot = magi_ipc::FrameReader::new(read)
                    .read::<magi_proto::HarnessEvent>()
                    .await
                    .expect("ordinary snapshot");
                assert!(
                    matches!(snapshot, magi_proto::HarnessEvent::SessionSnapshot { session, .. }
                    if session.as_str() == native)
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("ordinary host ready");
    let ordinary_pid = ordinary.id().expect("ordinary PID");
    assert_eq!(
        std::fs::read_link(format!("/proc/{ordinary_pid}/exe")).expect("ordinary executable"),
        fixture.binary
    );
    loading(&mut fixture, 2, &id).await;
    let refused = fixture.read().await;
    assert_eq!(refused["id"], 2);
    assert!(refused.get("error").is_some(), "{refused}");
    assert!(ordinary.try_wait().expect("old owner survives").is_none());
    assert_eq!(mind.asked(), 1);
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(ordinary_pid).expect("PID range"))
            .expect("PID"),
        rustix::process::Signal::Term,
    )
    .expect("stop ordinary owner");
    assert!(
        tokio::time::timeout(Duration::from_secs(10), ordinary.wait())
            .await
            .expect("ordinary cleanup")
            .expect("ordinary exit")
            .success()
    );
    loading(&mut fixture, 3, &id).await;
    loaded(&mut fixture, 3, &id).await;
    assert_eq!(mind.asked(), 1);
    prompt(&mut fixture, 4, &id, "ordinary writer released").await;
    finish(&mut fixture, 4).await;
    assert_eq!(mind.asked(), 2);
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_load_replays_tools_without_execution_and_rebinds_session_mcp() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = super::mcp::model("acp-load-mcp", "echo", json!({}));
    synthetic(&fixture, &mind);
    let installed = std::fs::read(fixture.root.join("c/magi/init.lua")).expect("config");
    let servers = |token: &str| {
        json!([{
            "name":"Reloaded editor tools", "command":"/bin/sh",
            "args":["-c",super::mcp::server(),"fixture","literal argument"],
            "env":[{"name":"ACP_FIXTURE_TOKEN","value":token}]
        }])
    };
    let response = fixture
        .call(
            2,
            "session/new",
            json!({
                "cwd":fixture.root.join("a"),"mcpServers":servers("original-token")
            }),
        )
        .await;
    let id = response["result"]["sessionId"].clone();
    assert!(id.is_string(), "{response}");
    prompt(&mut fixture, 3, &id, "call original session tool").await;
    let (before, approvals) = super::mcp::result(&mut fixture, &id, true).await;
    assert!(before.contains("original-token"));
    assert_eq!(approvals, 1);
    assert_eq!(mind.asked(), 2);
    let mut fixture = restart(fixture).await;
    fixture.send(json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{
        "sessionId":id,"cwd":fixture.root.join("a"),"mcpServers":servers("replacement-token")
    }})).await;
    let mut replay = String::new();
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 2, "no replayed permission requests");
            assert!(frame.get("result").is_some(), "{frame}");
            break;
        }
        assert_eq!(frame["method"], "session/update");
        assert_eq!(frame["params"]["sessionId"], id);
        replay.push_str(&frame.to_string());
    }
    assert!(replay.contains("original-token") && replay.contains("tool_call"));
    assert!(!replay.contains("replacement-token"));
    assert_eq!(mind.asked(), 2);
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("a/called")).expect("calls"),
        "called\n"
    );
    prompt(&mut fixture, 3, &id, "call replacement session tool").await;
    let (after, approvals) = super::mcp::result(&mut fixture, &id, true).await;
    assert!(after.contains("replacement-token"));
    assert_eq!(
        approvals, 1,
        "old allow-once permission does not survive reload"
    );
    assert_eq!(mind.asked(), 4);
    assert!(mind.asks()[3].contains("replacement-token"));
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("a/called")).expect("calls"),
        "called\ncalled\n"
    );
    assert_eq!(
        std::fs::read(fixture.root.join("c/magi/init.lua")).expect("config unchanged"),
        installed
    );
    fixture.close().await;
}
