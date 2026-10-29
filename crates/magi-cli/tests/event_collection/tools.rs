use super::*;
use std::{io::Write, net::TcpListener};

fn provider(listener: TcpListener, active: bool) -> std::thread::JoinHandle<()> {
    listener.set_nonblocking(true).expect("bounded listener");
    std::thread::spawn(move || {
        for round in 0..if active { 1 } else { 2 } {
            let mut accepted = None;
            wait(|| match listener.accept() {
                Ok((stream, _)) => {
                    accepted = Some(stream);
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
                Err(error) => panic!("provider accept: {error}"),
            });
            let mut http = accepted.expect("HTTP connection");
            http.set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            http.set_write_timeout(Some(Duration::from_secs(5)))
                .expect("write timeout");
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                assert!(header.len() < 65536);
                let mut byte = [0];
                http.read_exact(&mut byte).expect("HTTP header");
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).expect("header text");
            let length: usize = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().expect("body length"))
                })
                .expect("content length");
            assert!(length < 262144);
            let mut request = vec![0; length];
            http.read_exact(&mut request).expect("request");
            let request: Value = serde_json::from_slice(&request).expect("request JSON");
            if round == 0 {
                assert!(
                    request["tools"]
                        .as_array()
                        .expect("tools")
                        .iter()
                        .any(|tool| tool["function"]["name"] == "probe"),
                    "{request}"
                );
            } else {
                assert!(
                    request["messages"]
                        .to_string()
                        .contains("native-shutdown-result"),
                    "{request}"
                );
            }
            let delta = if round == 0 {
                serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"probe1","type":"function","function":{"name":"probe","arguments":"{}"}}]},"finish_reason":"tool_calls"}]})
            } else {
                serde_json::json!({"choices":[{"delta":{"content":"finished"},"finish_reason":"stop"}]})
            };
            let body = format!("data: {delta}\n\ndata: [DONE]\n\n");
            write!(http, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("provider reply");
        }
    })
}

fn scenario(binary: &Path, siblings: &[PathBuf], enabled: bool, active: bool) {
    let root = Scratch::new("evt", if active { "active" } else { "idle" }).settling();
    for dir in [
        "c/magi",
        "c/melchior",
        "c/casper",
        "r",
        "d",
        "s",
        "h",
        "t",
        "p/.git",
    ] {
        fs::create_dir_all(root.join(dir)).expect("directory");
    }
    fs::set_permissions(root.join("r"), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    let listener = TcpListener::bind("127.0.0.1:0").expect("synthetic provider");
    fs::write(root.join("c/melchior/providers.lua"), format!(
        r#"melchior.provider("synthetic", {{name="Synthetic", api="openai-completions", base_url="http://{}", auth={{kind="none"}}, models={{{{id="test",name="Synthetic",context_window=8192,max_tokens=128}}}}}})"#,
        listener.local_addr().expect("provider address")
    )).expect("catalog");
    fs::write(root.join("c/magi/init.lua"), format!(
        "magi.events = {enabled}\nmagi.melchior = {:?}\nmagi.memory = {:?}\nmagi.tools = {:?}\nmagi.model = 'synthetic/test'\nmagi.helpers = {{memory=false}}\nmagi.may_spawn = false\nmagi.allow = {{{{verb='run',anything=true}},{{verb='write',directory={:?}}}}}\n",
        siblings[0].to_str().expect("model"), siblings[1].to_str().expect("memory"), siblings[2].to_str().expect("tools"), root.join("p").to_str().expect("project")
    )).expect("configuration");
    let finish = if active {
        "exec sleep 30"
    } else {
        "printf native-shutdown-result"
    };
    fs::write(root.join("c/casper/tools.lua"), format!(r#"
casper.tool("probe", {{description="Native shutdown probe", needs="run", parameters={{type="object",properties={{}}}}, run=function()
 local result = casper.exec("sh", {{"-c", [[printf '%s' $$ > shell-pid
for f in /proc/$$/fd/*; do readlink "$f" || :; done > shell-fds
: > ready
n=0
while [ ! -f release ]; do n=$((n+1)); [ "$n" -lt 500 ] || exit 73; sleep 0.01; done
{finish}]]}})
 return {{said=result.out,failed=result.code ~= 0}}
end}})
"#)).expect("native tool");
    let peer = provider(listener, active);
    let socket = root.join("r/host.sock");
    let child = Command::new(binary)
        .args(["--headless", "--socket"])
        .arg(&socket)
        .arg("run the probe")
        .process_group(0)
        .env_clear()
        .current_dir(root.join("p"))
        .env("PATH", std::env::var_os("PATH").expect("path"))
        .env("HOME", root.join("h"))
        .env("TMPDIR", root.join("t"))
        .env("XDG_CONFIG_HOME", root.join("c"))
        .env("MELCHIOR_CONFIG", root.join("c/melchior"))
        .env("XDG_RUNTIME_DIR", root.join("r"))
        .env("XDG_DATA_HOME", root.join("d"))
        .env("XDG_STATE_HOME", root.join("s"))
        .env("MAGI_TOOLS_DOOR", "library")
        .stdin(Stdio::null())
        .stdout(File::create(root.join("out")).expect("stdout"))
        .stderr(File::create(root.join("err")).expect("stderr"))
        .spawn()
        .expect("selected Magi");
    let pid = child.id();
    let mut running = Running { child: Some(child) };
    wait(|| {
        assert!(
            running
                .child
                .as_mut()
                .expect("Magi")
                .try_wait()
                .expect("status")
                .is_none(),
            "{}",
            output(&root, "err")
        );
        root.join("p/ready").exists()
    });
    assert_eq!(
        fs::read_link(format!("/proc/{pid}/exe")).expect("Magi executable"),
        binary
    );
    let natives: Vec<_> = children(pid)
        .into_iter()
        .filter(|child| {
            fs::read_link(format!("/proc/{child}/exe")).is_ok_and(|exe| siblings.contains(&exe))
        })
        .collect();
    let casper = *natives
        .iter()
        .find(|child| {
            fs::read_link(format!("/proc/{child}/exe")).is_ok_and(|exe| exe == siblings[2])
        })
        .expect("native Casper daemon");
    let args = fs::read(format!("/proc/{casper}/cmdline")).expect("Casper argv");
    assert!(args.split(|byte| *byte == 0).any(|arg| arg == b"serve"));
    assert_eq!(
        args.split(|byte| *byte == 0)
            .any(|arg| arg == b"--event-bootstrap"),
        enabled
    );
    let daemon_socket = root.join(format!("r/casper/magi-{pid}-0.sock"));
    assert!(daemon_socket.exists());
    assert!(
        !fs::read_to_string(root.join("p/shell-fds"))
            .expect("shell descriptors")
            .contains("socket:")
    );
    let inner = fs::read_to_string(root.join("p/shell-pid")).expect("namespace shell PID");
    let mut shell = None;
    wait(|| {
        let mut descendants = children(casper);
        let mut at = 0;
        while let Some(pid) = descendants.get(at).copied() {
            assert!(descendants.len() < 64, "fixture process bound");
            if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) {
                let namespace_pid = status
                    .lines()
                    .find_map(|line| line.strip_prefix("NSpid:"))
                    .and_then(|line| line.split_whitespace().last());
                if namespace_pid == Some(inner.trim()) {
                    shell = Some(pid);
                    return true;
                }
                descendants.extend(children(pid));
            }
            at += 1;
        }
        false
    });
    let shell = shell.expect("host shell PID");
    fs::write(root.join("p/release"), b"release").expect("release tool");
    peer.join().expect("synthetic provider completed");
    wait(|| {
        let status = inspect(&socket);
        if enabled {
            assert_eq!(status["events"]["summary"]["counts"]["invalid"], 0);
            status["events"]["summary"]["remote_producers"]
                .as_u64()
                .expect("producers")
                >= 3
        } else {
            assert_eq!(status["events"], serde_json::json!({"state":"disabled"}));
            true
        }
    });
    let started = Instant::now();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(pid).expect("PID")).expect("positive PID"),
        rustix::process::Signal::Term,
    )
    .expect("graceful shutdown");
    wait(|| {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "shutdown waited for tool deadline"
        );
        running
            .child
            .as_mut()
            .expect("Magi")
            .try_wait()
            .expect("reap")
            .is_some_and(|status| {
                assert!(status.success(), "{}", output(&root, "err"));
                true
            })
    });
    running.child.take();
    for child in natives {
        assert!(
            !Path::new(&format!("/proc/{child}")).exists(),
            "native child remains: {child}"
        );
    }
    wait(|| {
        let gone = !Path::new(&format!("/proc/{shell}")).exists();
        assert!(
            gone || started.elapsed() < Duration::from_secs(3),
            "tool outlived shutdown"
        );
        gone
    });
    assert!(!daemon_socket.exists(), "owned daemon socket remains");
    assert!(!socket.exists(), "host socket remains");
}

#[test]
fn graceful_shutdown_reaps_native_tool_daemons_and_active_commands() {
    let mut siblings = Vec::new();
    for name in [
        "MELCHIOR_TEST_BINARY",
        "BALTHASAR_TEST_BINARY",
        "CASPER_TEST_BINARY",
    ] {
        let Some(selected) = std::env::var_os(name) else {
            magi_testkit::live::unavailable(name);
            return;
        };
        siblings.push(fs::canonicalize(selected).expect("selected sibling"));
    }
    let binary = fs::canonicalize(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
        .expect("selected Magi");
    for enabled in [false, true] {
        for active in [false, true] {
            scenario(&binary, &siblings, enabled, active);
        }
    }
}
