use super::*;

const SERVER: &str = r#"
set -eu
if [ "$FIXTURE_GUARDIAN_TEST" = 1 ]; then
  for signal in STOP KILL; do
    if kill -"$signal" "$PPID" 2>/dev/null; then
      : > guardian-signal-escaped
      exit 76
    fi
    printf '%s\n' "$signal" >> guardian-protected
  done
fi
hold() {
  printf '%s' $$ > mcp-pid
  "$FIXTURE_SHELL" -c '
    printf "%s" $$ > descendant-pid
    : > mcp-ready
    n=0
    while [ ! -f release ]; do n=$((n+1)); [ "$n" -lt 500 ] || exit 73; "$FIXTURE_SLEEP" 0.01; done
    : > descendant-completed
  ' &
  descendant=$!
}
while IFS= read -r line; do
  method=$(printf '%s' "$line" | "$FIXTURE_JQ" -r '.method // ""')
  printf '%s\n' "$method" >> requests
  id=$(printf '%s' "$line" | "$FIXTURE_JQ" -c '.id // null')
  case "$FIXTURE_STAGE:$method" in
    initialize:initialize|discovery:tools/list) hold; wait "$descendant" ;;
  esac
  case "$method" in
    initialize)
      protocol=$(printf '%s' "$line" | "$FIXTURE_JQ" -r '.params.protocolVersion')
      "$FIXTURE_JQ" -cn --argjson id "$id" --arg protocol "$protocol" '{jsonrpc:"2.0",id:$id,result:{protocolVersion:$protocol,capabilities:{tools:{}},serverInfo:{name:"containment",version:"1"}}}' ;;
    tools/list)
      "$FIXTURE_JQ" -cn --argjson id "$id" '{jsonrpc:"2.0",id:$id,result:{tools:[{name:"probe",description:"descriptor probe",inputSchema:{type:"object",properties:{}}}]}}' ;;
    tools/call)
      hold
      if [ "$FIXTURE_EARLY" = 1 ]; then
        n=0
        while [ ! -f reply-release ]; do n=$((n+1)); [ "$n" -lt 500 ] || exit 74; "$FIXTURE_SLEEP" 0.01; done
      else
        wait "$descendant"
      fi
      : > completed
      "$FIXTURE_JQ" -cn --argjson id "$id" '{jsonrpc:"2.0",id:$id,result:{content:[{type:"text",text:"contained-mcp-result"}]}}'
      [ "$FIXTURE_EARLY" != 1 ] || exit 0 ;;
    *) ;;
  esac
done
"#;

fn children(pid: u32) -> Vec<u32> {
    let mut children = Vec::new();
    if let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) {
        for task in tasks.flatten() {
            if let Ok(text) = fs::read_to_string(task.path().join("children")) {
                children.extend(
                    text.split_whitespace()
                        .filter_map(|pid| pid.parse::<u32>().ok()),
                );
            }
        }
    }
    children.sort_unstable();
    children.dedup();
    children
}

fn instance(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

fn descendant(parent: u32, namespace_pid: &str, executable: &Path) -> (u32, String) {
    let mut found = None;
    wait(|| {
        let mut tree = children(parent);
        let mut at = 0;
        while let Some(pid) = tree.get(at).copied() {
            assert!(tree.len() < 64, "bounded fixture descendants");
            let matching = fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("NSpid:"))
                    .and_then(|line| line.split_whitespace().last())
                    == Some(namespace_pid)
            });
            if matching
                && fs::read_link(format!("/proc/{pid}/exe"))
                    .is_ok_and(|actual| actual == executable)
            {
                found = Some((pid, instance(pid).expect("process instance")));
                return true;
            }
            tree.extend(children(pid));
            at += 1;
        }
        false
    });
    found.expect("selected external process")
}

fn serve_threads(pid: u32) -> Vec<u32> {
    fs::read_dir(format!("/proc/{pid}/task"))
        .expect("daemon tasks")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            (fs::read_to_string(entry.path().join("comm")).ok()?.trim() == "casper-serve")
                .then(|| entry.file_name().to_string_lossy().parse().ok())
                .flatten()
        })
        .collect()
}

fn cancel_waiting_peer(native: &mut Native, socket: &Path, workspace: &Path) {
    let original = serve_threads(native.child.id());
    assert_eq!(original.len(), 1, "one initializing/discovering connection");
    let before = fs::read(workspace.join("requests")).expect("original requests");
    let mut peer = UnixStream::connect(socket).expect("queued peer");
    peer.set_write_timeout(Some(Duration::from_secs(1)))
        .expect("write bound");
    let request = serde_json::to_vec(&json!({"call":"run","key":KEY,
        "args":[{"tool":"mcp__external__probe","args":{}}]}))
    .expect("queued call");
    peer.write_all(&u32::try_from(request.len()).expect("length").to_be_bytes())
        .expect("queued header");
    peer.write_all(&request).expect("queued body");
    let mut waiting = None;
    wait(|| {
        let tasks = serve_threads(native.child.id());
        let new: Vec<_> = tasks
            .into_iter()
            .filter(|tid| !original.contains(tid))
            .collect();
        if let [tid] = new.as_slice() {
            let path = format!("/proc/{}/task/{tid}/wchan", native.child.id());
            if fs::read_to_string(path).is_ok_and(|name| name.trim() == "hrtimer_nanosleep") {
                waiting = Some((*tid, instance(*tid).expect("waiting thread instance")));
                return true;
            }
        }
        false
    });
    let (tid, started) = waiting.expect("observed pool waiter");
    drop(peer);
    let until = Instant::now() + Duration::from_secs(1);
    while instance(tid).as_ref() == Some(&started) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_ne!(
        instance(tid).as_ref(),
        Some(&started),
        "cancelled waiter survived held pool"
    );
    assert_eq!(serve_threads(native.child.id()), original);
    assert_eq!(
        fs::read(workspace.join("requests")).expect("requests after cancellation"),
        before
    );
}

fn verify(binary: &Path, collector: &mut Collector, cases: &[(bool, &str)]) {
    let paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").expect("PATH")).collect();
    let shell = paths
        .iter()
        .map(|dir| dir.join("sh"))
        .find(|path| path.is_file())
        .expect("shell");
    let shell = fs::canonicalize(shell).expect("selected shell");
    let utility = |name: &str| {
        let path = paths
            .iter()
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
            .expect("fixture utility");
        let resolved = fs::canonicalize(path).expect("utility target");
        let named = resolved.with_file_name(name);
        assert_eq!(fs::canonicalize(&named).expect("named utility"), resolved);
        named
    };
    let environment =
        json!({"FIXTURE_SHELL":shell,"FIXTURE_JQ":utility("jq"),"FIXTURE_SLEEP":utility("sleep")});
    for &(bubblewrap, mode) in cases {
        let deadline = mode == "deadline";
        let disconnected = mode.ends_with("disconnect");
        let mut environment = environment.clone();
        environment["FIXTURE_GUARDIAN_TEST"] = json!(if bubblewrap { "0" } else { "1" });
        environment["FIXTURE_EARLY"] = json!(if mode == "leader-exit" { "1" } else { "0" });
        environment["FIXTURE_STAGE"] = json!(mode.strip_suffix("-disconnect").unwrap_or("call"));
        let fixture = Fixture::new(
            binary,
            if bubblewrap {
                "mcp-bwrap"
            } else {
                "mcp-landlock"
            },
        );
        let workspace = fixture.root.join("work");
        fs::create_dir(&workspace).expect("workspace");
        let script = workspace.join("external-mcp.sh");
        fs::write(&script, SERVER).expect("MCP fixture");
        let timeout = if deadline { 1 } else { 10 };
        let config = json!({"mcp_servers":{"external":{"command":shell,"args":[script],"env":environment,"needs":"run","timeout_secs":timeout}}});
        let mut command = fixture.command();
        command
            .current_dir(&workspace)
            .env("CASPER_CONFIGURE", config.to_string())
            .env("CASPER_JAIL", "1");
        if !bubblewrap {
            let paths: Vec<_> = paths
                .iter()
                .filter(|dir| !dir.join("bwrap").is_file())
                .collect();
            command.env("PATH", std::env::join_paths(paths).expect("Landlock PATH"));
        }
        let socket_path = PathBuf::from("/var/tmp/runtime/casper/mcp.sock");
        if disconnected {
            let runtime = Path::new("/var/tmp/runtime");
            fs::create_dir(runtime).expect("private namespace runtime");
            fs::set_permissions(runtime, fs::Permissions::from_mode(0o700)).expect("runtime mode");
            command.env("XDG_RUNTIME_DIR", runtime);
        }
        let args = if disconnected {
            vec!["serve", "--at", socket_path.to_str().expect("socket path")]
        } else {
            vec!["run"]
        };
        let mut native = fixture.spawn_command(collector, &args, command);
        let mut peer = if disconnected {
            writeln!(native.writer, "{KEY}").expect("daemon key");
            wait(|| fixture.output().contains('\n'));
            let ready: Value = serde_json::from_str(fixture.output().trim()).expect("daemon ready");
            assert_eq!(ready["event"], "listening", "{ready}");
            let mut peer = UnixStream::connect(&socket_path).expect("daemon peer");
            peer.set_write_timeout(Some(Duration::from_secs(2)))
                .expect("write bound");
            let body = serde_json::to_vec(&json!({"call":"run","key":KEY,
                "args":[{"tool":"mcp__external__probe","args":{}}]}))
            .expect("daemon request");
            peer.write_all(&u32::try_from(body.len()).expect("length").to_be_bytes())
                .expect("frame length");
            peer.write_all(&body).expect("frame body");
            Some(peer)
        } else {
            serde_json::to_writer(
                &mut native.writer,
                &json!({"tool":"mcp__external__probe","args":{}}),
            )
            .expect("call");
            native
                .writer
                .shutdown(std::net::Shutdown::Write)
                .expect("input eof");
            None
        };
        wait(|| {
            collector.poll();
            if let Some(status) = native.child.try_wait().expect("Casper state") {
                let mut diagnostic = String::new();
                File::open(fixture.root.join("stderr"))
                    .expect("stderr")
                    .take(8192)
                    .read_to_string(&mut diagnostic)
                    .expect("diagnostic");
                panic!(
                    "premature exit ({status}, bubblewrap={bubblewrap}): {} stderr={diagnostic}",
                    fixture.output()
                );
            }
            workspace.join("mcp-ready").exists()
        });
        if !bubblewrap {
            assert!(!workspace.join("guardian-signal-escaped").exists());
            assert_eq!(
                fs::read_to_string(workspace.join("guardian-protected"))
                    .expect("supervisor signal isolation"),
                "STOP\nKILL\n"
            );
        }
        let event = fs::read_to_string(fixture.root.join("event-target")).expect("event socket");
        let mut owned = Vec::new();
        for prefix in ["mcp", "descendant"] {
            let pid =
                fs::read_to_string(workspace.join(format!("{prefix}-pid"))).expect("namespace PID");
            let process = descendant(native.child.id(), pid.trim(), &shell);
            let fds: Vec<_> = fs::read_dir(format!("/proc/{}/fd", process.0))
                .expect("live descriptor directory")
                .map(|entry| {
                    fs::read_link(entry.expect("descriptor entry").path())
                        .expect("descriptor target")
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            assert!(
                fds.len() >= 3 && fds.iter().any(|fd| fd.starts_with("pipe:[")),
                "descriptor snapshot must observe live stdio for {prefix}: {fds:?}"
            );
            assert!(
                !fds.iter().any(|fd| fd == &event),
                "event channel escaped into {prefix}"
            );
            assert!(
                !fds.iter().any(|fd| fd.starts_with("socket:")),
                "private socket inherited by {prefix}: {fds:?}"
            );
            let parent_namespace = fs::read_link(format!("/proc/{}/ns/pid", native.child.id()))
                .expect("Casper PID namespace");
            let child_namespace =
                fs::read_link(format!("/proc/{}/ns/pid", process.0)).expect("MCP PID namespace");
            assert_eq!(
                child_namespace != parent_namespace,
                bubblewrap,
                "actual backend namespace for {prefix}"
            );
            owned.push(process);
        }
        if disconnected {
            if mode != "disconnect" {
                cancel_waiting_peer(&mut native, &socket_path, &workspace);
                assert!(
                    owned
                        .iter()
                        .all(|(pid, started)| instance(*pid).as_ref() == Some(started))
                );
            }
            drop(peer.take());
            let until = Instant::now() + Duration::from_secs(2);
            while owned
                .iter()
                .any(|(pid, started)| instance(*pid).as_ref() == Some(started))
                && Instant::now() < until
            {
                collector.poll();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                owned
                    .iter()
                    .all(|(pid, started)| instance(*pid).as_ref() != Some(started)),
                "MCP descendants survived peer disconnect: bubblewrap={bubblewrap}, owned={owned:?}"
            );
            assert!(native.child.try_wait().expect("daemon state").is_none());
            assert!(!workspace.join("descendant-completed").exists());
            assert!(!workspace.join("completed").exists());
            fs::write(workspace.join("release"), b"release").expect("recovery release");
            let mut recovery = UnixStream::connect(&socket_path).expect("recovery peer");
            recovery
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("recovery deadline");
            let reply = socket_reply(
                &mut recovery,
                json!({"call":"run","key":KEY,
                "args":[{"tool":"mcp__external__probe","args":{}}]}),
            );
            assert_eq!(reply["ok"], true, "{reply}");
            assert!(
                matches!(
                    reply["result"][0]["failed"],
                    Value::Null | Value::Bool(false)
                ),
                "{reply}"
            );
            let content: Value = serde_json::from_str(
                reply["result"][0]["said"]
                    .as_str()
                    .expect("MCP recovery text"),
            )
            .expect("MCP content");
            assert_eq!(content["content"][0]["text"], "contained-mcp-result");
            drop(recovery);
            native.child.kill().expect("stop daemon");
            native.child.wait().expect("reap daemon");
            native.lifetime.take();
            wait(|| {
                collector.poll();
                collector.summary().remote_producers == 0
            });
            continue;
        }
        if matches!(mode, "kill" | "terminate") {
            let pid = i32::try_from(native.child.id())
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .expect("Casper PID");
            let signal = if mode == "kill" {
                rustix::process::Signal::Kill
            } else {
                rustix::process::Signal::Term
            };
            rustix::process::kill_process(pid, signal).expect("signal owned Casper");
            wait(|| native.child.try_wait().expect("Casper wait").is_some());
            native.lifetime.take();
            let until = Instant::now() + Duration::from_secs(2);
            while owned
                .iter()
                .any(|(pid, started)| instance(*pid).as_ref() == Some(started))
                && Instant::now() < until
            {
                collector.poll();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                owned
                    .iter()
                    .all(|(pid, started)| instance(*pid).as_ref() != Some(started)),
                "MCP descendants survived abrupt Casper exit: bubblewrap={bubblewrap}, mode={mode}, owned={owned:?}"
            );
            assert!(!workspace.join("descendant-completed").exists());
            assert!(!workspace.join("completed").exists());
            assert!(!fixture.output().contains("contained-mcp-result"));
            wait(|| {
                collector.poll();
                collector.summary().remote_producers == 0
            });
            assert_eq!(collector.summary().retired.unknown_loss_producers, 1);
            continue;
        }
        if !deadline {
            let marker = if mode == "leader-exit" {
                "reply-release"
            } else {
                "release"
            };
            fs::write(workspace.join(marker), b"release").expect("release");
        }
        native.finish(collector);
        let output = fixture.output();
        let mut diagnostic = String::new();
        File::open(fixture.root.join("stderr"))
            .expect("stderr")
            .take(8192)
            .read_to_string(&mut diagnostic)
            .expect("diagnostic");
        let reply: Value = serde_json::from_str(output.trim()).expect("reply");
        assert_eq!(reply["ok"], true, "{output}");
        if deadline {
            assert_eq!(reply["result"][0]["failed"], true, "{output}");
            assert!(!output.contains("contained-mcp-result"));
            assert!(!workspace.join("completed").exists());
        } else {
            assert!(
                matches!(
                    reply["result"][0]["failed"],
                    Value::Null | Value::Bool(false)
                ),
                "bubblewrap={bubblewrap}: {output} stderr={diagnostic}"
            );
            assert!(
                reply["result"][0]["said"]
                    .as_str()
                    .expect("result")
                    .contains("contained-mcp-result")
            );
        }
        assert!(
            owned
                .iter()
                .all(|(pid, started)| instance(*pid).as_ref() != Some(started)),
            "MCP descendants survived completion: bubblewrap={bubblewrap}, mode={mode}, owned={owned:?}"
        );
        assert_eq!(
            workspace.join("descendant-completed").exists(),
            mode == "complete"
        );
    }
}

struct Probe(Child);

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(pid) = i32::try_from(self.0.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
        }
        let _ = self.0.wait();
    }
}

#[test]
fn selected_external_mcp_descendants_exclude_event_descriptors_in_both_jails() {
    isolated(
        "containment::selected_external_mcp_descendants_exclude_event_descriptors_in_both_jails",
        &[
            (true, "complete"),
            (false, "complete"),
            (true, "deadline"),
            (false, "deadline"),
            (true, "leader-exit"),
            (false, "leader-exit"),
        ],
    );
}

#[test]
fn selected_external_mcp_bubblewrap_cleans_up_after_casper_kill() {
    isolated(
        "containment::selected_external_mcp_bubblewrap_cleans_up_after_casper_kill",
        &[(true, "kill")],
    );
}

#[test]
fn selected_external_mcp_landlock_cleans_up_after_casper_kill() {
    isolated(
        "containment::selected_external_mcp_landlock_cleans_up_after_casper_kill",
        &[(false, "kill")],
    );
}

#[test]
fn selected_external_mcp_bubblewrap_cleans_up_after_casper_termination() {
    isolated(
        "containment::selected_external_mcp_bubblewrap_cleans_up_after_casper_termination",
        &[(true, "terminate")],
    );
}

#[test]
fn selected_external_mcp_landlock_cleans_up_after_casper_termination() {
    isolated(
        "containment::selected_external_mcp_landlock_cleans_up_after_casper_termination",
        &[(false, "terminate")],
    );
}

#[test]
fn selected_external_mcp_bubblewrap_cleans_up_after_peer_disconnect() {
    isolated(
        "containment::selected_external_mcp_bubblewrap_cleans_up_after_peer_disconnect",
        &[(true, "disconnect")],
    );
}

#[test]
fn selected_external_mcp_landlock_cleans_up_after_peer_disconnect() {
    isolated(
        "containment::selected_external_mcp_landlock_cleans_up_after_peer_disconnect",
        &[(false, "disconnect")],
    );
}

#[test]
fn selected_external_mcp_bubblewrap_cancels_initialization() {
    isolated(
        "containment::selected_external_mcp_bubblewrap_cancels_initialization",
        &[(true, "initialize-disconnect")],
    );
}

#[test]
fn selected_external_mcp_landlock_cancels_initialization() {
    isolated(
        "containment::selected_external_mcp_landlock_cancels_initialization",
        &[(false, "initialize-disconnect")],
    );
}

#[test]
fn selected_external_mcp_bubblewrap_cancels_discovery() {
    isolated(
        "containment::selected_external_mcp_bubblewrap_cancels_discovery",
        &[(true, "discovery-disconnect")],
    );
}

#[test]
fn selected_external_mcp_landlock_cancels_discovery() {
    isolated(
        "containment::selected_external_mcp_landlock_cancels_discovery",
        &[(false, "discovery-disconnect")],
    );
}

fn isolated(case: &str, cases: &[(bool, &str)]) {
    const ENV: &str = "MAGI_EVENT_MCP_CONTAINMENT";
    let Some(binary) = std::env::var_os("CASPER_TEST_BINARY") else {
        magi_testkit::live::unavailable("CASPER_TEST_BINARY for MCP containment");
        return;
    };
    if std::env::var(ENV).ok().as_deref() == Some(case) {
        assert_eq!(rustix::process::geteuid().as_raw(), 0);
        let binary = fs::canonicalize(binary).expect("selected Casper");
        let mut collector = Collector::new(events::enable_local().expect("root queue"));
        verify(&binary, &mut collector, cases);
        let summary = collector.close();
        assert_eq!(summary.counts.invalid, 0);
        assert_eq!(summary.retired.producers, cases.len() as u64);
        assert!(summary.counts.accepted >= 2);
        return;
    }
    use std::os::unix::{fs::MetadataExt, process::CommandExt};
    let root = Scratch::new("event-mcp", "namespace");
    for name in ["tmp", "var-tmp"] {
        fs::create_dir(root.join(name)).expect("namespace directory");
    }
    let backend = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|dir| dir.join("bwrap"))
        .find(|path| path.is_file())
        .expect("captured backend");
    let metadata = fs::metadata(&backend).expect("backend metadata");
    assert_eq!(metadata.mode() & 0o222, 0);
    assert_eq!(metadata.nlink(), 1);
    let mut command = Command::new(backend);
    command
        .process_group(0)
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--uid",
            "0",
            "--gid",
            "0",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--cap-add",
            "ALL",
            "--bind",
        ])
        .arg(&*root)
        .arg(&*root)
        .arg("--bind")
        .arg(root.join("var-tmp"))
        .arg("/var/tmp")
        .args(["--die-with-parent", "--new-session", "--"])
        .arg(std::env::current_exe().expect("test image"))
        .args(["--exact", case, "--nocapture"])
        .env(ENV, case)
        .env("TMPDIR", root.join("tmp"))
        .stdin(Stdio::null())
        .stdout(File::create(root.join("stdout")).expect("stdout"))
        .stderr(File::create(root.join("stderr")).expect("stderr"));
    let mut probe = Probe(command.spawn().expect("isolated test"));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = probe.0.try_wait().expect("test state") {
            let mut diagnostic = String::new();
            for name in ["stdout", "stderr"] {
                File::open(root.join(name))
                    .expect("diagnostic")
                    .take(65536)
                    .read_to_string(&mut diagnostic)
                    .expect("diagnostic text");
            }
            assert!(status.success(), "{diagnostic}");
            break;
        }
        assert!(Instant::now() < deadline, "isolated containment deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
