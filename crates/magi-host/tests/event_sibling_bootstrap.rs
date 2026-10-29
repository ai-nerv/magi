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
    net::{TcpListener, TcpStream},
    os::{
        fd::{AsFd, AsRawFd},
        unix::{fs::PermissionsExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[path = "event_sibling_bootstrap/memory_requests.rs"]
mod memory_requests;

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "native sibling deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn target(fd: impl AsFd) -> PathBuf {
    fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd())).expect("descriptor identity")
}

struct Fixture {
    root: Scratch,
    binary: PathBuf,
}

impl Fixture {
    fn new(binary: &Path, name: &str) -> Self {
        let root = Scratch::new("event-native", name);
        for name in [
            "home",
            "config/melchior",
            "runtime",
            "tmp",
            "data",
            "cache",
            "state",
        ] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).expect("directory");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("private mode");
        }
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
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("MELCHIOR_CONFIG", self.root.join("config/melchior"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .stdout(File::create(self.root.join("stdout")).expect("stdout"))
            .stderr(File::create(self.root.join("stderr")).expect("stderr"));
        command
    }

    fn output(&self, name: &str) -> String {
        let mut text = String::new();
        File::open(self.root.join(name))
            .expect("output")
            .take(65537)
            .read_to_string(&mut text)
            .expect("output bytes");
        assert!(text.len() <= 65536, "bounded output");
        text
    }

    fn spawn(&self, collector: &mut Collector, owner: Owner, mut command: Command) -> Native {
        let identity = || events::Id::generate().expect("lineage identity");
        let lineage = events::Lineage {
            turn_id: Some(identity()),
            ask_id: Some(identity()),
            ..events::Lineage::root(identity(), identity())
        };
        let opened = collector
            .open_bound(owner, lineage)
            .expect("bound native registration");
        let (input, writer) = UnixStream::pair().expect("native stdin");
        let original = target(&input);
        let handoff = bootstrap::prepare(opened.sender, input).expect("bootstrap");
        let packet = target(&handoff);
        let child = command
            .arg("--event-bootstrap")
            .stdin(Stdio::from(handoff))
            .spawn()
            .expect("selected native process");
        let mut native = Native {
            child,
            writer,
            lifetime: Some(opened.lifetime),
        };
        wait(|| {
            assert!(
                native.child.try_wait().expect("startup state").is_none(),
                "startup failed: {}",
                self.output("stderr")
            );
            fs::read_link(format!("/proc/{}/fd/0", native.child.id()))
                .ok()
                .as_ref()
                == Some(&original)
        });
        assert_eq!(
            fs::read_link(format!("/proc/{}/exe", native.child.id())).expect("actual executable"),
            self.binary
        );
        let fds: Vec<_> = fs::read_dir(format!("/proc/{}/fd", native.child.id()))
            .expect("native descriptors")
            .filter_map(|entry| fs::read_link(entry.ok()?.path()).ok())
            .collect();
        assert!(
            !fds.contains(&packet),
            "bootstrap descriptor survived restoration"
        );
        assert!(
            fds.iter()
                .any(|fd| fd != &original && fd.to_string_lossy().starts_with("socket:")),
            "event sender missing"
        );
        native
    }
}

struct Native {
    child: Child,
    writer: UnixStream,
    lifetime: Option<Lifetime>,
}

impl Native {
    fn finish(&mut self, collector: &mut Collector, success: bool) {
        wait(|| {
            collector.poll();
            self.child
                .try_wait()
                .expect("native wait")
                .is_some_and(|status| {
                    assert_eq!(status.success(), success, "native exit: {status}");
                    true
                })
        });
        self.lifetime.take();
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

fn socket_call(path: &Path, body: Value) -> Value {
    let mut socket = UnixStream::connect(path).expect("native socket");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read bound");
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write bound");
    let bytes = serde_json::to_vec(&body).expect("request");
    socket
        .write_all(&u32::try_from(bytes.len()).expect("length").to_be_bytes())
        .expect("length");
    socket.write_all(&bytes).expect("request body");
    let mut len = [0; 4];
    socket.read_exact(&mut len).expect("reply length");
    let len = u32::from_be_bytes(len) as usize;
    assert!(len <= 65536);
    let mut bytes = vec![0; len];
    socket.read_exact(&mut bytes).expect("reply body");
    serde_json::from_slice(&bytes).expect("reply json")
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("HTTP read bound");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("HTTP write bound");
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 16384);
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("HTTP header");
        bytes.push(byte[0]);
    }
    let header = String::from_utf8(bytes).expect("headers");
    let len = header
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().expect("body length"))
        })
        .expect("bounded body length");
    assert!(len <= 65536);
    let mut body = vec![0; len];
    stream.read_exact(&mut body).expect("HTTP request body");
    String::from_utf8(body).expect("body text")
}

fn melchior_ask(binary: &Path, collector: &mut Collector) {
    let fixture = Fixture::new(binary, "ask");
    let listener = TcpListener::bind("127.0.0.1:0").expect("synthetic provider");
    listener.set_nonblocking(true).expect("nonblocking accept");
    fs::write(fixture.root.join("config/melchior/providers.lua"), format!(r#"melchior.provider("synthetic", {{ name="Synthetic", api="openai-completions", base_url="http://{}", auth={{kind="none"}}, models={{{{id="test",name="Synthetic",context_window=8192,max_tokens=128}}}} }})"#, listener.local_addr().expect("provider address"))).expect("synthetic provider catalog");
    let mut command = fixture.command();
    command.arg("ask");
    let before = collector.counts().accepted;
    let mut native = fixture.spawn(collector, Owner::Melchior, command);
    serde_json::to_writer(&mut native.writer, &json!({"model":"synthetic/test", "context":{"messages":[{"role":"user","content":[{"type":"text","text":"private-input-canary"}]}]}})).expect("ask");
    native
        .writer
        .shutdown(std::net::Shutdown::Write)
        .expect("ask eof");
    let mut accepted = None;
    wait(|| {
        collector.poll();
        if accepted.is_none() {
            accepted = listener.accept().ok().map(|(stream, _)| stream);
        }
        assert!(
            native.child.try_wait().expect("ask state").is_none(),
            "ask failed: {} {}",
            fixture.output("stdout"),
            fixture.output("stderr")
        );
        accepted.is_some() && collector.counts().accepted > before
    });
    let mut http = accepted.expect("HTTP connection");
    assert!(read_request(&mut http).contains("private-input-canary"));
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"private-result\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";
    write!(http, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).expect("response headers");
    http.write_all(body).expect("response body");
    drop(http);
    drop(listener);
    native.finish(collector, true);
    let output = fixture.output("stdout");
    assert!(output.contains("private-result"), "{output}");
    assert!(
        output
            .lines()
            .any(|line| serde_json::from_str::<Value>(line).is_ok_and(|v| v["event"] == "stop")),
        "{output}"
    );
}

fn melchior_serve(binary: &Path, collector: &mut Collector) {
    let fixture = Fixture::new(binary, "serve");
    let mut command = fixture.command();
    command.args(["serve", "--project", "demo"]);
    let mut native = fixture.spawn(collector, Owner::Melchior, command);
    wait(|| fixture.output("stdout").contains('\n'));
    let first: Value = serde_json::from_str(
        fixture
            .output("stdout")
            .lines()
            .next()
            .expect("announcement"),
    )
    .expect("listening");
    assert_eq!(first["event"], "listening");
    let path = PathBuf::from(first["at"].as_str().expect("socket"));
    assert!(path.is_socket());
    writeln!(
        native.writer,
        "{}",
        json!({"event":"doing", "busy":true, "working_for":2})
    )
    .expect("status input");
    wait(|| {
        socket_call(&path, json!({"call":"status", "from":"demo/main/observer"}))["result"][0]["busy"]
            == true
    });
    native
        .writer
        .shutdown(std::net::Shutdown::Write)
        .expect("native parent eof");
    native.finish(collector, true);
    assert!(!path.exists(), "server socket not reaped");
}

fn balthasar_serve(binary: &Path, collector: &mut Collector) {
    let fixture = Fixture::new(binary, "memory");
    let store = fixture.root.join("memory.db");
    let mut command = fixture.command();
    command
        .args(["--no-config", "--store"])
        .arg(store)
        .args(["serve", "--instance", "events", "--tied"])
        .arg(std::process::id().to_string());
    let mut native = fixture.spawn(collector, Owner::Balthasar, command);
    wait(|| {
        fs::read_dir(format!("/proc/{}/task", native.child.id()))
            .expect("native threads")
            .filter_map(|entry| fs::read_to_string(entry.ok()?.path().join("status")).ok())
            .any(|status| {
                status.lines().any(|line| {
                    line.starts_with("Name:\t balthasar-event")
                        || line.starts_with("Name:\tbalthasar-event")
                }) && status
                    .lines()
                    .find_map(|line| line.strip_prefix("SigBlk:\t"))
                    .is_some_and(|mask| {
                        u64::from_str_radix(mask.trim(), 16).expect("signal mask") & (1 << 14) != 0
                    })
            })
    });
    writeln!(native.writer, "{KEY}").expect("original key input");
    let path = fixture.root.join("runtime/memory/api@events.sock");
    wait(|| path.exists());
    assert!(
        !path.with_extension("sock.key").exists(),
        "handed key must not be published"
    );
    let args = json!(["private-session", {"model":"synthetic", "window":8192, "reply":128}]);
    let refused = socket_call(&path, json!({"call":"layout", "args":args, "key":"wrong"}));
    assert_eq!(refused["ok"], false);
    let observed = socket_call(
        &path,
        json!({"call":"observe", "args":["private-session", {
            "cursor":0, "role":"user", "kind":"user", "text":"private-input-canary"
        }], "key":KEY}),
    );
    assert_eq!(observed["ok"], true, "{observed}");
    let before = collector.counts().accepted;
    let reply = socket_call(&path, json!({"call":"layout", "args":args, "key":KEY}));
    assert_eq!(reply["ok"], true, "{reply}");
    wait(|| {
        collector.poll();
        collector.counts().accepted > before
    });
    let pid = rustix::process::Pid::from_raw(i32::try_from(native.child.id()).expect("pid"))
        .expect("positive pid");
    rustix::process::kill_process(pid, rustix::process::Signal::Term).expect("graceful stop");
    native.finish(collector, true);
    assert!(!path.exists(), "signal cleanup bypassed");
}

fn balthasar_invalid_key(binary: &Path, collector: &mut Collector) {
    let fixture = Fixture::new(binary, "badkey");
    let store = fixture.root.join("memory.db");
    let mut command = fixture.command();
    command
        .args(["--no-config", "--store"])
        .arg(&store)
        .args(["serve", "--instance", "badkey"]);
    let before = collector.counts().statuses;
    let mut native = fixture.spawn(collector, Owner::Balthasar, command);
    writeln!(native.writer, "invalid-key").expect("malformed original key");
    native.finish(collector, false);
    wait(|| {
        collector.poll();
        collector.counts().statuses > before
    });
    assert!(fixture.output("stderr").contains("64 lowercase"));
    assert!(!fixture.root.join("runtime/memory/api@badkey.sock").exists());
    assert!(!store.exists(), "invalid startup must not open the store");
}

fn melchior_bind_failure(binary: &Path, collector: &mut Collector) {
    let fixture = Fixture::new(binary, "bound");
    let mut command = fixture.command();
    command
        .arg("serve")
        .env("MAGI_MELCHIOR_PROJECT", "demo")
        .env("MAGI_MELCHIOR_ID", "alpha-rho");
    let mut owner = fixture.spawn(collector, Owner::Melchior, command);
    wait(|| fixture.output("stdout").contains('\n'));
    let ready: Value = serde_json::from_str(
        fixture
            .output("stdout")
            .lines()
            .next()
            .expect("announcement"),
    )
    .expect("listening");
    let path = PathBuf::from(ready["at"].as_str().expect("bound socket"));
    let failed = Fixture::new(binary, "collision");
    let opened = collector
        .open(Owner::Melchior)
        .expect("failing native registration");
    let (input, writer) = UnixStream::pair().expect("original input");
    let bootstrap = bootstrap::prepare(opened.sender, input).expect("bootstrap");
    let before = collector.counts().statuses;
    let child = failed
        .command()
        .args(["serve", "--event-bootstrap"])
        .env("XDG_RUNTIME_DIR", fixture.root.join("runtime"))
        .env("MAGI_MELCHIOR_PROJECT", "demo")
        .env("MAGI_MELCHIOR_ID", "alpha-rho")
        .stdin(Stdio::from(bootstrap))
        .spawn()
        .expect("colliding native");
    let mut native = Native {
        child,
        writer,
        lifetime: Some(opened.lifetime),
    };
    native.finish(collector, false);
    wait(|| {
        collector.poll();
        collector.counts().statuses > before
    });
    assert!(
        failed
            .output("stderr")
            .contains(path.to_str().expect("socket text"))
    );
    assert!(
        path.is_socket(),
        "failed starter must not remove the existing server"
    );
    owner
        .writer
        .shutdown(std::net::Shutdown::Write)
        .expect("owner eof");
    owner.finish(collector, true);
    assert!(!path.exists());
}

trait SocketPath {
    fn is_socket(&self) -> bool;
}
impl SocketPath for PathBuf {
    fn is_socket(&self) -> bool {
        use std::os::unix::fs::FileTypeExt;
        fs::metadata(self).is_ok_and(|meta| meta.file_type().is_socket())
    }
}

#[test]
fn selected_siblings_preserve_native_input_reexec_signals_and_useful_work() {
    let (Some(melchior), Some(balthasar)) = (
        std::env::var_os("MELCHIOR_TEST_BINARY"),
        std::env::var_os("BALTHASAR_TEST_BINARY"),
    ) else {
        magi_testkit::live::unavailable("selected Melchior and Balthasar event binaries");
        return;
    };
    let melchior = fs::canonicalize(melchior).expect("selected Melchior");
    let balthasar = fs::canonicalize(balthasar).expect("selected Balthasar");
    let mut collector = Collector::new(events::enable_local().expect("root queue"));
    melchior_ask(&melchior, &mut collector);
    melchior_serve(&melchior, &mut collector);
    melchior_bind_failure(&melchior, &mut collector);
    balthasar_serve(&balthasar, &mut collector);
    balthasar_invalid_key(&balthasar, &mut collector);
    memory_requests::verify(&balthasar);
    wait(|| {
        collector.poll();
        collector.summary().remote_producers == 0
    });
    let summary = collector.close();
    assert_eq!(summary.counts.invalid, 0);
    assert_eq!(summary.retired.producers, 6);
    assert!(summary.counts.accepted >= 2);
    assert_eq!(
        summary.retired.final_confirmed + summary.retired.unknown_loss_producers,
        6
    );
}
