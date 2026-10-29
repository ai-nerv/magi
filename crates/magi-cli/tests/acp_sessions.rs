#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_model::scratch::Scratch;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct Fixture {
    binary: PathBuf,
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    output: BufReader<tokio::process::ChildStdout>,
    root: Scratch,
}

impl Fixture {
    async fn start() -> Option<Self> {
        let mut binaries = Vec::new();
        for (name, variable) in [
            ("casper", "CASPER_TEST_BINARY"),
            ("melchior", "MELCHIOR_TEST_BINARY"),
            ("balthasar", "BALTHASAR_TEST_BINARY"),
        ] {
            let Some(path) = std::env::var_os(variable)
                .map(PathBuf::from)
                .filter(|path| path.is_file())
            else {
                magi_testkit::live::unavailable(&format!("ACP native fixture needs {variable}"));
                return None;
            };
            binaries.push((name, path));
        }
        let Some(tools) = std::env::var_os("CASPER_TEST_TOOLS")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
        else {
            magi_testkit::live::unavailable("ACP native fixture needs CASPER_TEST_TOOLS");
            return None;
        };
        let root = Scratch::new("ac", "session").settling();
        for name in [
            "bin", "r", "c/magi", "c/casper", "d", "s", "h", "a/.git", "b/.git",
        ] {
            std::fs::create_dir_all(root.join(name)).expect("fixture directories");
        }
        use std::os::unix::fs::{PermissionsExt, symlink};
        std::fs::set_permissions(root.join("r"), std::fs::Permissions::from_mode(0o700))
            .expect("private runtime");
        for (name, path) in binaries {
            symlink(path, root.join("bin").join(name)).expect("selected binary");
        }
        std::fs::copy(tools, root.join("c/casper/tools.lua")).expect("selected tools");
        std::fs::write(
            root.join("c/magi/init.lua"),
            "magi.model = 'synthetic/unavailable'\n",
        )
        .expect("config");
        let search = std::env::var_os("PATH").expect("fixture tools path");
        let head = std::env::split_paths(&search)
            .map(|directory| directory.join("head"))
            .find(|path| path.is_file())
            .and_then(|path| std::fs::canonicalize(path).ok())
            .expect("selected core utilities");
        let shell = std::env::split_paths(&search)
            .map(|directory| directory.join("sh"))
            .find(|path| path.is_file())
            .and_then(|path| std::fs::canonicalize(path).ok())
            .expect("selected native shell");
        let path = std::env::join_paths([
            root.join("bin"),
            head.parent().expect("core utilities directory").to_owned(),
            shell.parent().expect("shell directory").to_owned(),
        ])
        .expect("fixture PATH");
        let binary = std::fs::canonicalize(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
            .expect("selected Magi executable");
        let mut child = tokio::process::Command::new(&binary)
            .arg("acp")
            .env_clear()
            .env("HOME", root.join("h"))
            .env("PATH", path)
            .env("XDG_CONFIG_HOME", root.join("c"))
            .env("XDG_DATA_HOME", root.join("d"))
            .env("XDG_STATE_HOME", root.join("s"))
            .env("XDG_RUNTIME_DIR", root.join("r"))
            .env("MAGI_API_SOCKET", root.join("must-not-attach"))
            .env("MAGI_MEMORY_INSTANCE", "must-not-share")
            .env("MAGI_MELCHIOR_SESSION", "must-not-share")
            .current_dir(&*root)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("ACP adapter");
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().expect("stdout"));
        let mut fixture = Self {
            binary,
            child,
            input,
            output,
            root,
        };
        let reply = fixture
            .call(1, "initialize", json!({"protocolVersion":1}))
            .await;
        assert!(reply.get("result").is_some(), "{reply}");
        assert_eq!(
            std::fs::read_link(format!(
                "/proc/{}/exe",
                fixture.child.id().expect("adapter PID")
            ))
            .expect("running adapter executable"),
            fixture.binary,
            "native ACP must exercise the selected executable"
        );
        Some(fixture)
    }

    async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        let response = self.read().await;
        assert_eq!(response["id"], id);
        response
    }

    async fn send(&mut self, request: Value) {
        let input = self.input.as_mut().expect("stdin open");
        input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("request");
    }

    async fn read(&mut self) -> Value {
        let mut bytes = Vec::new();
        let mut bounded = (&mut self.output).take(65_537);
        let size = tokio::time::timeout(
            Duration::from_secs(40),
            bounded.read_until(b'\n', &mut bytes),
        )
        .await
        .expect("response deadline")
        .expect("response");
        assert!(size > 0 && size < 65_536, "bounded response");
        serde_json::from_slice(&bytes).expect("JSON frame")
    }

    fn host_pids(&self) -> Vec<u32> {
        std::fs::read_dir("/proc")
            .expect("proc")
            .flatten()
            .filter_map(|entry| {
                let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
                let exe = std::fs::read_link(entry.path().join("exe")).ok()?;
                let cwd = std::fs::read_link(entry.path().join("cwd")).ok()?;
                (exe == self.binary && cwd.starts_with(&*self.root) && Some(pid) != self.child.id())
                    .then_some(pid)
            })
            .collect()
    }

    async fn close(self) -> Scratch {
        self.close_status(true).await
    }

    async fn close_status(mut self, success: bool) -> Scratch {
        self.input.take();
        let waited = tokio::time::timeout(Duration::from_secs(25), self.child.wait()).await;
        if waited.is_err() {
            let _ = self.child.kill().await;
        }
        assert_eq!(
            waited
                .expect("adapter shutdown deadline")
                .expect("adapter exit")
                .success(),
            success
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let leaks: Vec<_> = std::fs::read_dir("/proc")
                .expect("proc")
                .flatten()
                .filter(|entry| {
                    std::fs::read_link(entry.path().join("cwd"))
                        .is_ok_and(|cwd| cwd.starts_with(&*self.root))
                })
                .collect();
            if leaks.is_empty() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "leaked owned processes: {:?}",
                leaks
                    .iter()
                    .map(|entry| (
                        entry.file_name(),
                        std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            std::fs::read_dir(self.root.join("r/magi"))
                .expect("runtime")
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with("acp-")),
            "session runtime directories cleaned"
        );
        self.root
    }
}

#[path = "acp_sessions/children.rs"]
mod children;
#[path = "acp_sessions/display.rs"]
mod display;
#[path = "acp_sessions/loading.rs"]
mod loading;
#[path = "acp_sessions/mcp.rs"]
mod mcp;
#[path = "acp_sessions/permissions.rs"]
mod permissions;
#[path = "acp_sessions/settings.rs"]
mod settings;
#[path = "acp_sessions/tool_lifecycle.rs"]
mod tool_lifecycle;
#[path = "acp_sessions/turns.rs"]
mod turns;

#[tokio::test]
async fn native_acp_sessions_keep_distinct_roots_and_cleanup_owned_hosts() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let first = fixture
        .call(
            2,
            "session/new",
            json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
        )
        .await;
    let second = fixture
        .call(
            3,
            "session/new",
            json!({"cwd":fixture.root.join("b"),"mcpServers":[]}),
        )
        .await;
    assert!(first["result"]["sessionId"].is_string(), "{first}");
    assert!(second["result"]["sessionId"].is_string(), "{second}");
    assert_ne!(first["result"]["sessionId"], second["result"]["sessionId"]);
    let hosts = fixture.host_pids();
    assert_eq!(hosts.len(), 2);
    for pid in &hosts {
        let environment =
            std::fs::read(format!("/proc/{pid}/environ")).expect("native environment");
        let variables: Vec<_> = environment.split(|byte| *byte == 0).collect();
        assert!(variables.contains(&b"CASPER_JAIL=1".as_slice()));
        for forbidden in [
            b"MAGI_API_SOCKET=".as_slice(),
            b"MAGI_MEMORY_INSTANCE=",
            b"MAGI_MELCHIOR_SESSION=",
        ] {
            assert!(
                variables.iter().all(|value| !value.starts_with(forbidden)),
                "no inherited session authority"
            );
        }
    }
    let roots: std::collections::BTreeSet<_> = hosts
        .iter()
        .map(|pid| std::fs::read_link(format!("/proc/{pid}/cwd")).expect("host cwd"))
        .collect();
    assert_eq!(
        roots,
        [fixture.root.join("a"), fixture.root.join("b")]
            .into_iter()
            .collect()
    );
    assert_eq!(
        std::fs::read_link(format!(
            "/proc/{}/cwd",
            fixture.child.id().expect("adapter pid")
        ))
        .expect("adapter cwd"),
        *fixture.root
    );
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_mcp_settings_are_private_session_local_and_literal() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let init = std::fs::read(fixture.root.join("c/magi/init.lua")).expect("config before");
    let response = fixture.call(2, "session/new", json!({"cwd":fixture.root.join("a"),"mcpServers":[{"name":"Editor tools λ","command":"/not-started/server","args":["$(not executed)"],"env":[{"name":"TOKEN","value":"test-secret"}]}]})).await;
    assert!(response["result"]["sessionId"].is_string(), "{response}");
    let paths: Vec<_> = std::fs::read_dir(fixture.root.join("r/magi"))
        .expect("runtime")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("acp-"))
        .map(|entry| entry.path())
        .collect();
    assert_eq!(paths.len(), 1);
    assert!(
        !paths[0].join("bootstrap.json").exists(),
        "startup handoff is unlinked before spawn"
    );
    for pid in fixture.host_pids() {
        assert_eq!(
            std::fs::read_link(format!("/proc/{pid}/fd/0")).expect("child stdin"),
            Path::new("/dev/null")
        );
    }
    assert_eq!(
        std::fs::read(fixture.root.join("c/magi/init.lua")).expect("config after"),
        init
    );
    assert!(!response.to_string().contains("test-secret"));
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_refuses_invalid_sessions_before_spawning_and_recovers() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let bad = fixture
        .call(2, "session/new", json!({"cwd":"relative","mcpServers":[]}))
        .await;
    assert_eq!(bad["error"]["code"], -32602);
    assert!(fixture.host_pids().is_empty());
    let missing = fixture
        .call(
            3,
            "session/new",
            json!({"cwd":fixture.root.join("missing"),"mcpServers":[]}),
        )
        .await;
    assert_eq!(missing["error"]["code"], -32000);
    assert!(fixture.host_pids().is_empty());
    let recovered = fixture
        .call(
            4,
            "session/new",
            json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
        )
        .await;
    assert!(recovered.get("result").is_some(), "{recovered}");
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_session_limit_does_not_spawn_a_fifth_host() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mut ids = std::collections::BTreeSet::new();
    for id in 2..6 {
        let response = fixture
            .call(
                id,
                "session/new",
                json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
            )
            .await;
        assert!(
            ids.insert(
                response["result"]["sessionId"]
                    .as_str()
                    .expect("session ID")
                    .to_owned()
            )
        );
    }
    let response = fixture
        .call(
            6,
            "session/new",
            json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
        )
        .await;
    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(fixture.host_pids().len(), 4);
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_disconnect_during_stalled_startup_reaps_the_child() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let config = fixture.root.join("c/magi/init.lua");
    std::fs::remove_file(&config).expect("remove fixture config");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &config,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("blocking fixture config");
    let request = json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":fixture.root.join("a"),"mcpServers":[]}});
    fixture
        .input
        .as_mut()
        .expect("stdin")
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("request");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while fixture.host_pids().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "native child started"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_disconnect_does_not_stop_another_adapters_host() {
    let Some(mut owner) = Fixture::start().await else {
        return;
    };
    let Some(mut other) = Fixture::start().await else {
        return;
    };
    for fixture in [&mut owner, &mut other] {
        let response = fixture
            .call(
                2,
                "session/new",
                json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
            )
            .await;
        assert!(response.get("result").is_some(), "{response}");
    }
    let other_pid = other.host_pids()[0];
    owner.close().await;
    assert!(other.child.try_wait().expect("other status").is_none());
    assert!(Path::new(&format!("/proc/{other_pid}")).exists());
    let response = other
        .call(
            3,
            "session/new",
            json!({"cwd":other.root.join("b"),"mcpServers":[]}),
        )
        .await;
    assert!(response.get("result").is_some(), "{response}");
    other.close().await;
}

#[tokio::test]
async fn native_acp_parent_death_during_config_load_cannot_orphan_a_host() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let config = fixture.root.join("c/magi/init.lua");
    std::fs::remove_file(&config).expect("remove fixture config");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &config,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("blocking config");
    let request = json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":fixture.root.join("a"),"mcpServers":[]}});
    fixture
        .input
        .as_mut()
        .expect("stdin")
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("request");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let host = loop {
        if let Some(pid) = fixture.host_pids().into_iter().find(|pid| {
            std::fs::read_link(format!("/proc/{pid}/fd/0"))
                .is_ok_and(|path| path == Path::new("/dev/null"))
        }) {
            break pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "child armed its parent-death guard"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    fixture.child.kill().await.expect("kill adapter");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while Path::new(&format!("/proc/{host}")).exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "orphaned native startup process {host}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for entry in std::fs::read_dir(fixture.root.join("r/magi"))
        .expect("runtime")
        .flatten()
    {
        if entry.file_name().to_string_lossy().starts_with("acp-") {
            assert!(
                !entry.path().join("bootstrap.json").exists(),
                "no named startup secrets survive an adapter crash"
            );
        }
    }
}
