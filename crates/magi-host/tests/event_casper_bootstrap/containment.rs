use super::*;

const SERVER: &str = r#"
set -eu
while IFS= read -r line; do
  method=$(printf '%s' "$line" | "$FIXTURE_JQ" -r '.method // ""')
  id=$(printf '%s' "$line" | "$FIXTURE_JQ" -c '.id // null')
  case "$method" in
    initialize)
      protocol=$(printf '%s' "$line" | "$FIXTURE_JQ" -r '.params.protocolVersion')
      "$FIXTURE_JQ" -cn --argjson id "$id" --arg protocol "$protocol" '{jsonrpc:"2.0",id:$id,result:{protocolVersion:$protocol,capabilities:{tools:{}},serverInfo:{name:"containment",version:"1"}}}' ;;
    tools/list)
      "$FIXTURE_JQ" -cn --argjson id "$id" '{jsonrpc:"2.0",id:$id,result:{tools:[{name:"probe",description:"descriptor probe",inputSchema:{type:"object",properties:{}}}]}}' ;;
    tools/call)
      printf '%s' $$ > mcp-pid
      "$FIXTURE_SHELL" -c '
        printf "%s" $$ > descendant-pid
        : > mcp-ready
        n=0
        while [ ! -f release ]; do n=$((n+1)); [ "$n" -lt 500 ] || exit 73; "$FIXTURE_SLEEP" 0.01; done
      ' &
      descendant=$!
      wait "$descendant"
      "$FIXTURE_JQ" -cn --argjson id "$id" '{jsonrpc:"2.0",id:$id,result:{content:[{type:"text",text:"contained-mcp-result"}]}}' ;;
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

fn verify(binary: &Path, collector: &mut Collector) {
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
    for bubblewrap in [true, false] {
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
        let config = json!({"mcp_servers":{"external":{"command":shell,"args":[script],"env":environment,"needs":"run","timeout_secs":10}}});
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
        let mut native = fixture.spawn_command(collector, &["run"], command);
        serde_json::to_writer(
            &mut native.writer,
            &json!({"tool":"mcp__external__probe","args":{}}),
        )
        .expect("call");
        native
            .writer
            .shutdown(std::net::Shutdown::Write)
            .expect("input eof");
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
        fs::write(workspace.join("release"), b"release").expect("release");
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
        wait(|| {
            owned
                .iter()
                .all(|(pid, started)| instance(*pid).as_ref() != Some(started))
        });
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
    const CASE: &str =
        "containment::selected_external_mcp_descendants_exclude_event_descriptors_in_both_jails";
    const ENV: &str = "MAGI_EVENT_MCP_CONTAINMENT";
    let Some(binary) = std::env::var_os("CASPER_TEST_BINARY") else {
        magi_testkit::live::unavailable("CASPER_TEST_BINARY for MCP containment");
        return;
    };
    if std::env::var(ENV).ok().as_deref() == Some(CASE) {
        assert_eq!(rustix::process::geteuid().as_raw(), 0);
        let binary = fs::canonicalize(binary).expect("selected Casper");
        let mut collector = Collector::new(events::enable_local().expect("root queue"));
        verify(&binary, &mut collector);
        let summary = collector.close();
        assert_eq!(summary.counts.invalid, 0);
        assert_eq!(summary.retired.producers, 2);
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
        .args(["--exact", CASE, "--nocapture"])
        .env(ENV, CASE)
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
