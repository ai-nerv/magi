use super::*;
use std::os::unix::net::UnixDatagram;

struct Peer(Option<Child>);

impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
    let (read, write) = UnixDatagram::pair().expect("event channel");
    for fd in [&read, &write] {
        fd.set_nonblocking(true).expect("nonblocking events");
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
    let frame = serde_json::to_vec(&json!({"schema":2,"kind":"native_event_bootstrap","program":"casper", "producer":events::Id::generate().expect("producer"),"lineage":root})).expect("packet");
    assert!(frame.len() <= 512);
    let descriptors = [write.as_fd(), input.as_fd()];
    let mut storage = [0; rustix::cmsg_space!(ScmRights(2))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    assert!(control.push(SendAncillaryMessage::ScmRights(&descriptors)));
    assert_eq!(
        net::sendmsg(
            &parent,
            &[std::io::IoSlice::new(&frame)],
            &mut control,
            SendFlags::NOSIGNAL
        )
        .expect("packet send"),
        frame.len()
    );
    let path = fixture.root.join("runtime/casper/requests.sock");
    let child = fixture
        .command()
        .args(["serve", "--event-bootstrap", "--at"])
        .arg(&path)
        .stdin(Stdio::from(child))
        .spawn()
        .expect("native daemon");
    let mut native = Native {
        child,
        writer,
        lifetime: None,
    };
    writeln!(native.writer, "{KEY}").expect("key");
    wait(|| fixture.output().contains('\n'));
    let ready: Value = serde_json::from_str(fixture.output().trim()).expect("listening");
    assert_eq!(ready["request_context"], 1);
    assert_eq!(
        fs::read_link(format!("/proc/{}/exe", native.child.id())).expect("executable"),
        fixture.binary
    );
    (native, read, root)
}

fn context(root: events::Lineage, turn: events::Id) -> Value {
    json!({"schema":1,"run_id":root.run_id,"agent_id":root.agent_id,"turn_id":turn,"ask_id":null})
}

fn body(context: Option<Value>) -> Value {
    let mut body = json!({"call":"run","key":KEY,"args":[{"tool":"probe","args":{"command":"sleep 0.02; printf private-useful-result", "event_context":{"turn_id":"forged-model-value"}}}]});
    if let Some(context) = context {
        body["event_context"] = context;
    }
    body
}

fn invoke(path: &Path, body: Value) {
    let mut socket = UnixStream::connect(path).expect("socket");
    invoke_socket(&mut socket, body);
}

fn invoke_socket(socket: &mut UnixStream, body: Value) {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read deadline");
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write deadline");
    let reply = socket_reply(socket, body);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"][0]["said"], "private-useful-result");
}

fn observed(read: &UnixDatagram, root: events::Lineage, count: usize) -> Vec<Value> {
    let mut facts = Vec::new();
    wait(|| {
        loop {
            let mut bytes = [0; 4097];
            match read.recv(&mut bytes) {
                Ok(n) => {
                    assert!(n <= 4096);
                    let value: Value = serde_json::from_slice(&bytes[..n]).expect("event json");
                    if value.get("status").is_some() {
                        continue;
                    }
                    assert_eq!(value["program"], "casper");
                    assert_eq!(value["correlation"]["run_id"], json!(root.run_id));
                    assert_eq!(value["correlation"]["agent_id"], json!(root.agent_id));
                    assert_eq!(value["fact"]["kind"], "tool");
                    assert!(!String::from_utf8_lossy(&bytes[..n]).contains("private"));
                    facts.push(value);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("receive: {error}"),
            }
        }
        facts.len() >= count
    });
    assert_eq!(facts.len(), count);
    facts
}

#[test]
fn other_peer() {
    if std::env::var_os("MAGI_REQUEST_OTHER_PEER").is_none() {
        return;
    }
    let value: Value = serde_json::from_reader(std::io::stdin().take(8192)).expect("fixture input");
    invoke(
        Path::new(value["path"].as_str().expect("path")),
        value["body"].clone(),
    );
}

pub(super) fn verify(binary: &Path) {
    let fixture = Fixture::new(binary, "request-context");
    let (_native, read, root) = start(&fixture);
    let path = fixture.root.join("runtime/casper/requests.sock");
    let first = events::Id::generate().expect("turn");
    let second = events::Id::generate().expect("turn");
    let mut refused = body(Some(context(root, first)));
    refused["key"] = json!("wrong");
    refused["args"][0]["args"]["command"] = json!("printf forbidden > forbidden");
    let mut socket = UnixStream::connect(&path).expect("key refusal socket");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("refusal deadline");
    assert_eq!(socket_reply(&mut socket, refused)["ok"], false);
    assert!(!fixture.root.join("forbidden").exists());
    for turn in [first, second] {
        invoke_socket(&mut socket, body(Some(context(root, turn))));
        for event in observed(&read, root, 2) {
            assert_eq!(event["correlation"]["turn_id"], json!(turn));
        }
    }
    let mut wrong_root = context(root, first);
    wrong_root["run_id"] = json!(events::Id::generate().expect("forged root"));
    for value in [
        None,
        Some(wrong_root),
        Some(json!({"private":"x".repeat(1024)})),
    ] {
        invoke_socket(&mut socket, body(Some(context(root, first))));
        for event in observed(&read, root, 2) {
            assert_eq!(event["correlation"]["turn_id"], json!(first));
        }
        invoke_socket(&mut socket, body(value));
        for event in observed(&read, root, 2) {
            assert!(event["correlation"]["turn_id"].is_null());
        }
    }
    invoke_socket(&mut socket, body(Some(context(root, second))));
    for event in observed(&read, root, 2) {
        assert_eq!(event["correlation"]["turn_id"], json!(second));
    }
    drop(socket);
    let peer = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "requests::other_peer", "--nocapture"])
        .env("MAGI_REQUEST_OTHER_PEER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("other peer");
    let mut peer = Peer(Some(peer));
    serde_json::to_writer(
        peer.0
            .as_mut()
            .expect("peer")
            .stdin
            .take()
            .expect("peer input"),
        &json!({"path":path,"body":body(Some(context(root,first)))}),
    )
    .expect("peer request");
    wait(|| {
        peer.0
            .as_mut()
            .expect("peer")
            .try_wait()
            .expect("peer status")
            .is_some()
    });
    let output = peer
        .0
        .take()
        .expect("peer")
        .wait_with_output()
        .expect("peer output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    for event in observed(&read, root, 2) {
        assert!(event["correlation"]["turn_id"].is_null());
    }
    std::thread::scope(|scope| {
        for turn in [first, second] {
            let path = &path;
            scope.spawn(move || invoke(path, body(Some(context(root, turn)))));
        }
    });
    let mut calls = std::collections::BTreeMap::new();
    for event in observed(&read, root, 4) {
        let correlation = &event["correlation"];
        let turn = correlation["turn_id"].clone();
        assert!(turn == json!(first) || turn == json!(second));
        calls
            .entry(
                correlation["tool_call_id"]
                    .as_str()
                    .expect("tool id")
                    .to_owned(),
            )
            .or_insert_with(Vec::new)
            .push(turn);
    }
    assert_eq!(calls.len(), 2);
    for turns in calls.values() {
        assert_eq!(turns.len(), 2);
        assert_eq!(turns.first(), turns.last());
    }
    assert_ne!(
        calls.values().next().expect("first").first(),
        calls.values().last().expect("last").first()
    );
}
