#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_model::scratch::Scratch;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::Read,
    os::unix::{fs::PermissionsExt, net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[path = "event_collection/tools.rs"]
mod tools;

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "normal event startup deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Running {
    child: Option<Child>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = i32::try_from(child.id())
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
            }
            let _ = child.wait();
        }
    }
}

fn output(root: &Path, name: &str) -> String {
    let mut text = String::new();
    File::open(root.join(name))
        .expect("output")
        .take(65537)
        .read_to_string(&mut text)
        .expect("text");
    assert!(text.len() <= 65536);
    text
}

fn inspect(path: &Path) -> Value {
    let mut stream = UnixStream::connect(path).expect("host socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read deadline");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("write deadline");
    magi_ipc::blocking::FrameWriter::new(&mut stream)
        .write_blocking(&magi_proto::UiCommand::Inspect)
        .expect("inspect");
    magi_ipc::blocking::FrameReader::new(stream.take(16388))
        .read_blocking()
        .expect("bounded native status")
}

fn children(pid: u32) -> Vec<u32> {
    let mut children = Vec::new();
    for task in fs::read_dir(format!("/proc/{pid}/task"))
        .expect("tasks")
        .flatten()
    {
        if let Ok(text) = fs::read_to_string(task.path().join("children")) {
            children.extend(
                text.split_whitespace()
                    .filter_map(|pid| pid.parse::<u32>().ok()),
            );
        }
    }
    children.sort_unstable();
    children.dedup();
    children
}

fn scenario(binary: &Path, siblings: &[PathBuf], enabled: bool, legacy: bool) {
    let root = Scratch::new("evn", if enabled { "on" } else { "off" }).settling();
    for dir in ["c/magi", "r", "d", "s", "h", "t", "p/.git"] {
        fs::create_dir_all(root.join(dir)).expect("directory");
    }
    fs::set_permissions(root.join("r"), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    let program = if legacy {
        let wrapper = root.join("legacy");
        fs::write(&wrapper, format!("#!/bin/sh\nif [ \"$1\" = event-capabilities ]; then printf x >> '{}'; printf '%s\\n' '{{\"ok\":false}}'; exit 0; fi\nexec '{}' \"$@\"\n", root.join("queries").display(), siblings[0].display())).expect("legacy wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).expect("executable");
        wrapper
    } else {
        siblings[0].clone()
    };
    fs::write(root.join("c/magi/init.lua"), format!(
        "magi.events = {enabled}\nmagi.melchior = {:?}\nmagi.memory = {:?}\nmagi.tools = {:?}\nmagi.model = 'synthetic/unavailable'\n",
        program.to_str().expect("model path"), siblings[1].to_str().expect("memory path"), siblings[2].to_str().expect("tools path")
    )).expect("machine configuration");
    let socket = root.join("r/host.sock");
    let child = Command::new(binary)
        .args(["--headless", "--socket"])
        .arg(&socket)
        .process_group(0)
        .env_clear()
        .current_dir(root.join("p"))
        .env("PATH", std::env::var_os("PATH").expect("path"))
        .env("HOME", root.join("h"))
        .env("TMPDIR", root.join("t"))
        .env("XDG_CONFIG_HOME", root.join("c"))
        .env("XDG_RUNTIME_DIR", root.join("r"))
        .env("XDG_DATA_HOME", root.join("d"))
        .env("XDG_STATE_HOME", root.join("s"))
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
                .expect("child")
                .try_wait()
                .expect("startup")
                .is_none(),
            "{}",
            output(&root, "err")
        );
        socket.exists() && !output(&root, "out").is_empty()
    });
    assert_eq!(
        fs::read_link(format!("/proc/{pid}/exe")).expect("Magi executable"),
        binary
    );
    let mut natives = Vec::new();
    for child in children(pid) {
        let actual = fs::read_link(format!("/proc/{child}/exe")).expect("sibling executable");
        if siblings[..2].contains(&actual) {
            natives.push(child);
        }
    }
    assert_eq!(
        natives.len(),
        2,
        "both selected native siblings must be owned"
    );
    let expected = if legacy { 1 } else { 2 };
    wait(|| {
        let details = inspect(&socket);
        let _: magi_proto::monitoring::AgentDetails =
            serde_json::from_value(details.clone()).expect("existing reader compatibility");
        let events = &details["events"];
        if !enabled {
            assert_eq!(events, &serde_json::json!({"state":"disabled"}));
            return true;
        }
        assert_eq!(events["state"], "collecting");
        assert_eq!(
            events["unobserved_launches"].as_u64().unwrap_or(0),
            u64::from(legacy)
        );
        assert_eq!(events["summary"]["counts"]["invalid"], 0);
        events["summary"]["remote_producers"] == expected
            && events["summary"]["counts"]["statuses"]
                .as_u64()
                .expect("status count")
                >= expected
    });
    for child in &natives {
        let args = fs::read(format!("/proc/{child}/cmdline")).expect("native arguments");
        let bootstrap = args
            .split(|byte| *byte == 0)
            .any(|arg| arg == b"--event-bootstrap");
        let actual = fs::read_link(format!("/proc/{child}/exe")).expect("native executable");
        assert_eq!(bootstrap, enabled && !(legacy && actual == siblings[0]));
    }
    if legacy {
        assert_eq!(
            fs::read(root.join("queries")).unwrap_or_default(),
            if enabled {
                b"x".as_slice()
            } else {
                b"".as_slice()
            }
        );
    }
    let pid =
        rustix::process::Pid::from_raw(i32::try_from(pid).expect("pid")).expect("positive pid");
    rustix::process::kill_process(pid, rustix::process::Signal::Term).expect("graceful shutdown");
    wait(|| {
        running
            .child
            .as_mut()
            .expect("child")
            .try_wait()
            .expect("reap")
            .is_some_and(|status| {
                assert!(status.success(), "{}", output(&root, "err"));
                true
            })
    });
    running.child.take();
    for pid in natives {
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "sibling not reaped"
        );
    }
    assert!(!socket.exists(), "host socket remains");
}

#[test]
fn normal_launches_opt_in_preserve_old_siblings_and_reap_before_shutdown() {
    let mut siblings = Vec::new();
    for name in [
        "MELCHIOR_TEST_BINARY",
        "BALTHASAR_TEST_BINARY",
        "CASPER_TEST_BINARY",
    ] {
        let Some(path) = std::env::var_os(name).and_then(|path| fs::canonicalize(path).ok()) else {
            magi_testkit::live::unavailable(name);
            return;
        };
        siblings.push(path);
    }
    let binary = fs::canonicalize(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
        .expect("selected Magi");
    scenario(&binary, &siblings, false, true);
    scenario(&binary, &siblings, true, false);
    scenario(&binary, &siblings, true, true);
}
