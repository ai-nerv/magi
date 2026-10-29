use super::*;
use std::{cell::Cell, os::unix::net::UnixDatagram, sync::OnceLock};

static ROOT: OnceLock<events::Lineage> = OnceLock::new();
thread_local! { static TURN: Cell<Option<events::Id>> = const { Cell::new(None) }; }

fn current() -> Option<magi_ipc::family::observing::Context> {
    let root = ROOT.get()?;
    magi_ipc::family::observing::Context::new(
        String::from(root.run_id?),
        String::from(root.agent_id?),
        Some(String::from(TURN.get()?)),
        None,
    )
}

fn start(fixture: &Fixture) -> (Native, UnixDatagram, events::Lineage) {
    use rustix::net::{
        self, AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketFlags,
        SocketType, sockopt,
    };
    let root = events::Lineage::root(
        events::Id::generate().expect("run"),
        events::Id::generate().expect("agent"),
    );
    let (read, write) = UnixDatagram::pair().expect("events");
    for fd in [&read, &write] {
        fd.set_nonblocking(true).expect("nonblocking");
        sockopt::set_socket_send_buffer_size(fd, 16384).expect("send cap");
        sockopt::set_socket_recv_buffer_size(fd, 16384).expect("receive cap");
    }
    let (parent, child) = net::socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .expect("bootstrap");
    let (input, writer) = UnixStream::pair().expect("input");
    let packet = serde_json::to_vec(&json!({"schema":2,"kind":"native_event_bootstrap","program":"balthasar","producer":events::Id::generate().expect("producer"),"lineage":root})).expect("packet");
    assert!(packet.len() <= 512);
    let descriptors = [write.as_fd(), input.as_fd()];
    let mut storage = [0; rustix::cmsg_space!(ScmRights(2))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    assert!(control.push(SendAncillaryMessage::ScmRights(&descriptors)));
    assert_eq!(
        net::sendmsg(
            &parent,
            &[std::io::IoSlice::new(&packet)],
            &mut control,
            SendFlags::NOSIGNAL
        )
        .expect("send"),
        packet.len()
    );
    let child = fixture
        .command()
        .args(["--no-config", "--store"])
        .arg(fixture.root.join("memory.db"))
        .args(["serve", "--instance", "requests", "--event-bootstrap"])
        .stdin(Stdio::from(child))
        .spawn()
        .expect("native memory");
    let mut native = Native {
        child,
        writer,
        lifetime: None,
    };
    writeln!(native.writer, "{KEY}").expect("key");
    let path = fixture.root.join("runtime/memory/api@requests.sock");
    wait(|| {
        assert!(
            native.child.try_wait().expect("startup").is_none(),
            "{}",
            fixture.output("stderr")
        );
        path.exists()
    });
    assert_eq!(
        fs::read_link(format!("/proc/{}/exe", native.child.id())).expect("executable"),
        fixture.binary
    );
    (native, read, root)
}

fn layout() -> Value {
    json!(["private-session", {"model":"synthetic","window":8192,"reply":128,"event_context":{"turn_id":"forged-model-value"}}])
}

fn observed(read: &UnixDatagram, root: events::Lineage, turn: Option<events::Id>) {
    let mut facts = Vec::new();
    wait(|| {
        loop {
            let mut bytes = [0; 4097];
            match read.recv(&mut bytes) {
                Ok(n) => {
                    assert!(n <= 4096);
                    let event: Value = serde_json::from_slice(&bytes[..n]).expect("event");
                    if event.get("status").is_some() {
                        continue;
                    }
                    assert_eq!(event["program"], "balthasar");
                    assert_eq!(event["correlation"]["run_id"], json!(root.run_id));
                    assert_eq!(event["correlation"]["agent_id"], json!(root.agent_id));
                    assert_eq!(event["correlation"]["turn_id"], json!(turn));
                    assert_eq!(event["fact"]["kind"], "layout");
                    assert!(!String::from_utf8_lossy(&bytes[..n]).contains("private"));
                    facts.push(event);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("events: {error}"),
            }
        }
        !facts.is_empty()
    });
    assert_eq!(facts.len(), 1);
}

struct Peer(Option<Child>);
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn other_peer() {
    if std::env::var_os("MAGI_MEMORY_NATIVE_OTHER").is_none() {
        return;
    }
    let request: Value = serde_json::from_reader(std::io::stdin().take(8192)).expect("input");
    let reply = socket_call(
        Path::new(request["path"].as_str().expect("path")),
        request["body"].clone(),
    );
    assert_eq!(reply["ok"], true, "{reply}");
}

pub(super) fn verify(binary: &Path) {
    let fixture = Fixture::new(binary, "memory-requests");
    let (mut native, read, root) = start(&fixture);
    ROOT.set(root).expect("root once");
    assert!(magi_ipc::family::observing::install(current));
    let path = fixture.root.join("runtime/memory/api@requests.sock");
    magi_ipc::family::hold_key(&path, KEY);
    let verbs = socket_call(&path, json!({"call":"verbs","key":KEY}));
    assert_eq!(verbs["request_context"], 1);
    assert_eq!(
        socket_call(
            &path,
            json!({"call":"layout","args":layout(),"key":"wrong"})
        )["ok"],
        false
    );
    let reply = socket_call(
        &path,
        json!({"call":"observe","args":["private-session",{"cursor":0,"role":"user","kind":"user","text":"private-canary"}],"key":KEY}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let mut client = magi_ipc::family::blocking::Family::dial(&path).expect("memory client");
    client.call("verbs", vec![]).expect("negotiate");
    let first = events::Id::generate().expect("turn");
    let second = events::Id::generate().expect("turn");
    for turn in [Some(first), Some(second), None, Some(first)] {
        TURN.set(turn);
        let result = client
            .call("layout", layout().as_array().expect("args").clone())
            .expect("useful layout");
        assert!(result[0]["id"].is_string());
        observed(&read, root, turn);
    }
    magi_lua::name_session("private-session", Some(&path));
    let script = format!(
        "local h = assert(magi.stream.connect({:?}, 2000))\n\
         local r = assert(h:call({{call='layout', args={{'private-session', {{model='synthetic', window=8192, reply=128}}}},\
         event_context={{schema=1, run_id={:?}, agent_id={:?}, turn_id={:?}}}}}, {{deadline_ms=2000}}))\n\
         assert(r.ok and r.result[1].id)\n\
         h:close()\n\
         magi.answer = 'useful-layout'",
        path.to_string_lossy(),
        String::from(root.run_id.expect("run")),
        String::from(root.agent_id.expect("agent")),
        String::from(second),
    );
    for turn in [Some(first), None, Some(first)] {
        TURN.set(turn);
        let mut engine = magi_lua::Engine::new();
        engine
            .run(&script, "memory-context.lua")
            .expect("Lua layout");
        engine.harvest();
        assert_eq!(engine.config().string("answer"), Some("useful-layout"));
        observed(&read, root, turn);
    }
    let mut context = json!({"schema":1,"run_id":root.run_id,"agent_id":root.agent_id,"turn_id":first,"ask_id":null});
    let mut wrong = context.clone();
    wrong["run_id"] = json!(events::Id::generate().expect("wrong root"));
    for bad in [wrong, json!({"private":"x".repeat(1024)})] {
        let reply = socket_call(
            &path,
            json!({"call":"layout","args":layout(),"key":KEY,"event_context":bad}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        observed(&read, root, None);
    }
    context["turn_id"] = json!(second);
    let child = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "memory_requests::other_peer"])
        .env("MAGI_MEMORY_NATIVE_OTHER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("peer");
    let mut child = Peer(Some(child));
    serde_json::to_writer(child.0.as_mut().expect("peer").stdin.take().expect("stdin"), &json!({"path":path,"body":{"call":"layout","args":layout(),"key":KEY,"event_context":context}})).expect("request");
    wait(|| {
        child
            .0
            .as_mut()
            .expect("peer")
            .try_wait()
            .expect("status")
            .is_some()
    });
    let output = child
        .0
        .take()
        .expect("peer")
        .wait_with_output()
        .expect("output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    observed(&read, root, None);
    drop(client);
    let pid = rustix::process::Pid::from_raw(i32::try_from(native.child.id()).expect("pid"))
        .expect("positive pid");
    rustix::process::kill_process(pid, rustix::process::Signal::Term).expect("stop");
    wait(|| native.child.try_wait().expect("reap").is_some());
    assert!(!path.exists());
}
