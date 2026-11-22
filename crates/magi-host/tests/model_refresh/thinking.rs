use super::*;
use magi_model::ThinkingLevel;
use std::os::unix::{fs::DirBuilderExt, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = i32::try_from(self.0.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn isolated(case: &str) -> bool {
    const CHILD: &str = "MAGI_THINKING_FIXTURE";
    if std::env::var(CHILD).as_deref() == Ok(case) {
        return false;
    }
    let dir = Scratch::new("magi-thinking", "isolated");
    for name in ["home", "config", "cache", "data", "state", "run"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir.join(name))
            .expect("private directory");
    }
    let mut child = Process(
        Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", case, "--nocapture"])
            .process_group(0)
            .env_clear()
            .env(CHILD, case)
            .env("HOME", dir.join("home"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_RUNTIME_DIR", dir.join("run"))
            .env("PATH", std::env::var_os("PATH").expect("fixture utilities"))
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("isolated worker"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.0.try_wait().expect("worker status") {
            assert!(status.success(), "{case}: {status}");
            return true;
        }
        assert!(Instant::now() < deadline, "{case}: worker deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn event(
    reader: &mut FrameReader<OwnedReadHalf>,
    wanted: impl Fn(&HarnessEvent) -> bool,
) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = reader.read().await.expect("host event");
            if wanted(&event) {
                return event;
            }
        }
    })
    .await
    .expect("bounded event")
}

async fn changed(reader: &mut FrameReader<OwnedReadHalf>, name: &str, level: &str) {
    let actual = event(reader, |event| {
        matches!(event, HarnessEvent::ModelChanged { .. })
    })
    .await;
    assert!(
        matches!(actual, HarnessEvent::ModelChanged {model:Some(model),thinking:Some(thinking),..}
        if model.name == name && thinking == level)
    );
}

async fn turn(reader: &mut FrameReader<OwnedReadHalf>, writer: &mut FrameWriter<OwnedWriteHalf>) {
    writer
        .write(&UiCommand::SubmitPrompt {
            text: "fixture turn".into(),
            aside: String::new(),
            images: Vec::new(),
        })
        .await
        .expect("prompt");
    event(
        reader,
        |event| matches!(event, HarnessEvent::AssistantDelta { text, .. } if text == "ok"),
    )
    .await;
    event(reader, |event| {
        matches!(
            event,
            HarnessEvent::StatusChanged {
                status: magi_proto::AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn selected_effort_reaches_all_clients_snapshots_persistence_and_requests() {
    if isolated("thinking::selected_effort_reaches_all_clients_snapshots_persistence_and_requests")
    {
        return;
    }
    let dir = Scratch::new("magi-thinking", "session");
    let mind = magi_testkit::Mind::answering("thinking-session", "ok");
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.mind = mind.program().display().to_string();
    catalog.memory = "/fixture-no-real-memory".into();
    catalog.tooling.program = "/fixture-no-real-tools".into();
    catalog.cwd = dir.to_path_buf();
    catalog.wants.thinking = Some(ThinkingLevel::Low);
    catalog.cards = ["one", "two"]
        .into_iter()
        .map(|name| {
            serde_json::from_value(json!({"id":format!("fixture/{name}"),"provider":"fixture",
            "name":name,"api":"openai-completions","ready":true,"reasons":true,
            "context_window":100000,"max_output":1000,
            "capabilities":{"thinking_levels":["low","xhigh"]}}))
            .expect("card")
        })
        .collect();
    let backend = catalog.backend("fixture/one").expect("backend");
    let mut session = magi_host::open_session(1, "thinking");
    session.set_thinking("low".into());
    let path = dir.join("s.sock");
    let task = tokio::spawn(magi_host::serve_catalog(
        magi_ipc::bind(&path).await.expect("listener"),
        session,
        Some(backend),
        catalog,
    ));
    let (mut first, mut writer) = attach(&path).await;
    let (mut second, _writer) = attach(&path).await;
    writer
        .write(&UiCommand::SetThinking {
            level: "xhigh".into(),
        })
        .await
        .expect("thinking");
    changed(&mut first, "fixture/one", "xhigh").await;
    changed(&mut second, "fixture/one", "xhigh").await;
    let (_, _, snapshot) = attached(&path).await;
    assert!(matches!(snapshot, HarnessEvent::SessionSnapshot {thinking,..} if thinking == "xhigh"));
    assert_eq!(
        magi_host::remember::of(dir.to_str().expect("cwd"))
            .thinking
            .as_deref(),
        Some("xhigh")
    );
    turn(&mut first, &mut writer).await;
    let asks = mind.asks();
    let ask: serde_json::Value = serde_json::from_str(&asks[0]).expect("first ask");
    assert_eq!(ask["wants"]["thinking"], "xhigh");
    writer
        .write(&UiCommand::SetThinking {
            level: "max".into(),
        })
        .await
        .expect("unsupported");
    let refused = event(&mut first, |event| {
        matches!(event, HarnessEvent::Refused { .. })
    })
    .await;
    assert!(
        matches!(refused, HarnessEvent::Refused {message,..} if message.contains("not selectable"))
    );
    let (_, _, snapshot) = attached(&path).await;
    assert!(matches!(snapshot, HarnessEvent::SessionSnapshot {thinking,..} if thinking == "xhigh"));
    assert_eq!(
        magi_host::remember::of(dir.to_str().expect("cwd"))
            .thinking
            .as_deref(),
        Some("xhigh")
    );
    writer
        .write(&UiCommand::SetModel {
            name: "fixture/two".into(),
        })
        .await
        .expect("model");
    changed(&mut first, "fixture/two", "low").await;
    changed(&mut second, "fixture/two", "low").await;
    let (_, _, snapshot) = attached(&path).await;
    assert!(matches!(snapshot, HarnessEvent::SessionSnapshot {thinking,..} if thinking == "low"));
    turn(&mut first, &mut writer).await;
    let asks = mind.asks();
    assert_eq!(asks.len(), 2);
    let ask: serde_json::Value = serde_json::from_str(&asks[1]).expect("second ask");
    assert_eq!(ask["model"], "fixture/two");
    assert_eq!(ask["wants"]["thinking"], "low");
    let chosen = magi_host::remember::of(dir.to_str().expect("cwd"));
    assert_eq!(chosen.model.as_deref(), Some("fixture/two"));
    assert_eq!(chosen.thinking.as_deref(), Some("low"));
    task.abort();
    let _ = task.await;
}
