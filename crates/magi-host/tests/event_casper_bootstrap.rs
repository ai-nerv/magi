#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::events::{
    self,
    collection::{Collector, Lifetime, Owner, bootstrap},
};
use magi_testkit::Scratch;
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsFd, AsRawFd},
        unix::{fs::PermissionsExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[path = "event_casper_bootstrap/containment.rs"]
mod containment;
#[path = "event_casper_bootstrap/requests.rs"]
mod requests;
const TOOLS: &str = r#"
casper.tool("probe", {description="private-probe", parameters={}, run=function(args)
  local done = casper.exec("sh", {"-c", args.command})
  return {said=done.out, failed=done.code ~= 0}
end})
"#;
const PROBE: &str = r#"
printf '%s' "$$" > shell-pid
for f in /proc/$$/fd/*; do readlink "$f" || :; done > shell-fds
: > shell-ready
n=0
while [ ! -f release ]; do
  n=$((n + 1)); [ "$n" -lt 500 ] || exit 73
  sleep 0.01
done
printf private-useful-result
"#;

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "native Casper deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fd_target(fd: impl AsFd) -> PathBuf {
    fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd())).expect("fd target")
}

struct Fixture {
    root: Scratch,
    binary: PathBuf,
}

impl Fixture {
    fn new(binary: &Path, name: &str) -> Self {
        let root = Scratch::new("event-casper", name);
        for dir in [
            "home",
            "config/casper",
            "runtime",
            "cache",
            "data",
            "state",
            "tmp",
        ] {
            let path = root.join(dir);
            fs::create_dir_all(&path).expect("private fixture directory");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("mode");
        }
        fs::write(root.join("config/casper/tools.lua"), TOOLS).expect("tools");
        Self {
            root,
            binary: binary.to_owned(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .env_clear()
            .current_dir(&*self.root)
            .env("PATH", std::env::var_os("PATH").expect("PATH"))
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .stdout(File::create(self.root.join("stdout")).expect("stdout"))
            .stderr(File::create(self.root.join("stderr")).expect("stderr"));
        command
    }

    fn output(&self) -> String {
        let mut out = String::new();
        File::open(self.root.join("stdout"))
            .expect("stdout")
            .take(8193)
            .read_to_string(&mut out)
            .expect("output");
        assert!(out.len() <= 8192, "bounded native output");
        out
    }

    fn spawn(&self, collector: &mut Collector, args: &[&str]) -> Native {
        self.spawn_command(collector, args, self.command())
    }

    fn spawn_command(
        &self,
        collector: &mut Collector,
        args: &[&str],
        mut command: Command,
    ) -> Native {
        let identity = || events::Id::generate().expect("lineage identity");
        let lineage = events::Lineage {
            turn_id: Some(identity()),
            ask_id: Some(identity()),
            ..events::Lineage::root(identity(), identity())
        };
        let opened = collector
            .open_bound(Owner::Casper, lineage)
            .expect("bound native registration");
        let (input, writer) = UnixStream::pair().expect("original input");
        let original = fd_target(&input);
        let handoff = bootstrap::prepare(opened.sender, input).expect("bootstrap packet");
        let bootstrap_target = fd_target(&handoff);
        command
            .args(args)
            .arg("--event-bootstrap")
            .stdin(Stdio::from(handoff));
        let child = command.spawn().expect("selected Casper");
        let native = Native {
            child,
            writer,
            lifetime: Some(opened.lifetime),
        };
        wait(|| {
            fs::read_link(format!("/proc/{}/fd/0", native.child.id()))
                .ok()
                .as_ref()
                == Some(&original)
        });
        assert_eq!(
            fs::read_link(format!("/proc/{}/exe", native.child.id())).expect("actual executable"),
            self.binary
        );
        let sockets: Vec<_> = fs::read_dir(format!("/proc/{}/fd", native.child.id()))
            .expect("native descriptors")
            .filter_map(|entry| fs::read_link(entry.ok()?.path()).ok())
            .filter(|link| link.to_string_lossy().starts_with("socket:") && *link != original)
            .collect();
        assert_eq!(sockets.len(), 1, "exactly one native event descriptor");
        assert!(
            !sockets.contains(&bootstrap_target),
            "bootstrap stdin was replaced"
        );
        fs::write(
            self.root.join("event-target"),
            sockets[0].as_os_str().as_encoded_bytes(),
        )
        .expect("target");
        native
    }

    fn release(&self, collector: &mut Collector, accepted_before: u64) {
        wait(|| {
            collector.poll();
            self.root.join("shell-ready").exists() && collector.counts().accepted > accepted_before
        });
        let fds = fs::read_to_string(self.root.join("shell-fds")).expect("shell fds");
        let target = fs::read_to_string(self.root.join("event-target")).expect("event target");
        assert!(
            !fds.lines().any(|fd| fd == target),
            "event descriptor escaped to shell"
        );
        fs::write(self.root.join("release"), b"release").expect("release tool");
    }

    fn shell_reaped(&self) {
        let pid = fs::read_to_string(self.root.join("shell-pid")).expect("shell pid");
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "tool shell survived"
        );
    }
}

struct Native {
    child: Child,
    writer: UnixStream,
    lifetime: Option<Lifetime>,
}

impl Native {
    fn finish(&mut self, collector: &mut Collector) {
        wait(|| {
            collector.poll();
            self.child
                .try_wait()
                .expect("native wait")
                .is_some_and(|status| {
                    assert!(status.success(), "native exit: {status}");
                    true
                })
        });
        self.lifetime.take();
        wait(|| {
            collector.poll();
            collector.summary().remote_producers == 0
        });
        assert!(!Path::new(&format!("/proc/{}", self.child.id())).exists());
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.lifetime.take();
    }
}

fn per_call(binary: &Path, collector: &mut Collector, admitted: bool) {
    let fixture = Fixture::new(binary, if admitted { "admitted" } else { "run" });
    let accepted = collector.counts().accepted;
    let args = if admitted {
        vec!["run", "--file-admission"]
    } else {
        vec!["run"]
    };
    let mut native = fixture.spawn(collector, &args);
    if admitted {
        wait(|| fixture.output().contains("\n"));
        assert_eq!(
            fixture.output(),
            "{\"event\":\"file-admission\",\"revision_contract\":1}\n"
        );
    }
    serde_json::to_writer(
        &mut native.writer,
        &json!({"tool":"probe","args":{"command":PROBE}}),
    )
    .expect("native call");
    native
        .writer
        .shutdown(std::net::Shutdown::Write)
        .expect("input eof");
    fixture.release(collector, accepted);
    native.finish(collector);
    let output = fixture.output();
    let reply: Value =
        serde_json::from_str(output.lines().last().expect("reply")).expect("reply json");
    assert_eq!(reply["ok"], true, "{output}");
    assert_eq!(reply["result"][0]["said"], "private-useful-result");
    fixture.shell_reaped();
}

fn socket_reply(socket: &mut UnixStream, body: Value) -> Value {
    let body = serde_json::to_vec(&body).expect("request");
    socket
        .write_all(&u32::try_from(body.len()).expect("length").to_be_bytes())
        .expect("length");
    socket.write_all(&body).expect("body");
    let mut len = [0; 4];
    socket.read_exact(&mut len).expect("reply length");
    let len = u32::from_be_bytes(len) as usize;
    assert!(len <= 8192);
    let mut bytes = vec![0; len];
    socket.read_exact(&mut bytes).expect("reply");
    serde_json::from_slice(&bytes).expect("reply json")
}

fn daemon(binary: &Path, collector: &mut Collector) {
    let fixture = Fixture::new(binary, "serve");
    let path = fixture.root.join("runtime/casper/event.sock");
    let mut native = fixture.spawn(
        collector,
        &["serve", "--at", path.to_str().expect("socket path")],
    );
    writeln!(native.writer, "{KEY}").expect("original daemon key");
    wait(|| fixture.output().contains("\n"));
    let ready: Value = serde_json::from_str(fixture.output().trim()).expect("ready");
    assert_eq!(ready["event"], "listening");
    assert_eq!(ready["file_admission"], 1);
    let mut socket = UnixStream::connect(path).expect("daemon socket");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read deadline");
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write deadline");
    assert_eq!(
        socket_reply(
            &mut socket,
            json!({"call":"tools", "args":[], "key":"wrong"})
        )["ok"],
        false
    );
    assert_eq!(
        socket_reply(&mut socket, json!({"call":"tools", "args":[], "key":KEY}))["ok"],
        true
    );
    fs::write(fixture.root.join("release"), b"release").expect("release");
    let accepted = collector.counts().accepted;
    let reply = socket_reply(
        &mut socket,
        json!({"call":"run", "key":KEY, "args":[{"tool":"probe", "args":{"command":PROBE}}]}),
    );
    assert_eq!(reply["result"][0]["said"], "private-useful-result");
    fixture.release(collector, accepted);
    fixture.shell_reaped();
    drop(socket);
    native.child.kill().expect("terminate owned daemon");
    native.child.wait().expect("reap daemon");
    native.lifetime.take();
    wait(|| {
        collector.poll();
        collector.summary().remote_producers == 0
    });
    assert!(collector.summary().retired.unknown_loss_producers >= 1);
}

#[test]
fn selected_casper_accepts_native_handoff_without_changing_run_admission_or_daemon_input() {
    let Some(binary) = std::env::var_os("CASPER_TEST_BINARY") else {
        magi_testkit::live::unavailable("CASPER_TEST_BINARY for native event bootstrap");
        return;
    };
    let binary = fs::canonicalize(binary).expect("selected Casper executable");
    let refused = Fixture::new(&binary, "invalid-bootstrap");
    let input = refused.root.join("input");
    fs::write(
        &input,
        serde_json::to_vec(&json!({"tool":"probe","args":{"command":"touch must-not-execute"}}))
            .expect("call"),
    )
    .expect("input");
    let mut child = refused
        .command()
        .args(["run", "--event-bootstrap"])
        .stdin(File::open(input).expect("input"))
        .spawn()
        .expect("invalid bootstrap child");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().expect("wait refusal").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("refusal deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let reply: Value = serde_json::from_str(refused.output().trim()).expect("refusal");
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"], "invalid native event bootstrap");
    assert!(!refused.root.join("must-not-execute").exists());
    let mut collector = Collector::new(events::enable_local().expect("root queue"));
    per_call(&binary, &mut collector, false);
    per_call(&binary, &mut collector, true);
    daemon(&binary, &mut collector);
    let summary = collector.close();
    assert_eq!(summary.counts.invalid, 0);
    assert_eq!(summary.retired.producers, 3);
    assert!(summary.counts.accepted >= 3);
    assert!(
        summary.incomplete(),
        "killed daemon must not claim confirmed final loss"
    );
    requests::verify(&binary);
}
