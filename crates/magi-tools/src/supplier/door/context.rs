use super::{over_socket_context, tests::call, tests::framed};
use crate::{Uncancelled, supplier::launch};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

static QUERIES: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(true);
const ID: &str = "0123456789abcdef0123456789abcdef";

fn context() -> Option<launch::Context> {
    QUERIES.fetch_add(1, Ordering::SeqCst);
    ACTIVE.load(Ordering::SeqCst).then(|| {
        launch::Context::new(ID.into(), ID.into(), Some(ID.into()), None).expect("context")
    })
}

struct Isolated(Option<Child>);

impl Drop for Isolated {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn isolated() -> bool {
    const CHILD: &str = "MAGI_SUPPLIER_CONTEXT_TEST";
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "supplier::door::context::only_negotiated_requests_capture_current_host_context",
        ])
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated test");
    let mut child = Isolated(Some(child));
    let deadline = Instant::now() + Duration::from_secs(20);
    while child
        .0
        .as_mut()
        .expect("child")
        .try_wait()
        .expect("status")
        .is_none()
    {
        assert!(Instant::now() < deadline, "isolated context deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child
        .0
        .take()
        .expect("child")
        .wait_with_output()
        .expect("output");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    true
}

fn exchange(observed: bool) -> Value {
    let dir = crate::family::FamilyScratch::new("context").expect("scratch");
    let path = dir.join("context.sock");
    let listener = UnixListener::bind(&path).expect("listener");
    listener.set_nonblocking(true).expect("bounded accept");
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "accept deadline");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read deadline");
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("write deadline");
        let mut header = [0; 4];
        socket.read_exact(&mut header).expect("header");
        let length = u32::from_be_bytes(header) as usize;
        assert!((1..=8192).contains(&length));
        let mut body = vec![0; length];
        socket.read_exact(&mut body).expect("request");
        let reply = json!({"ok":true,"family":1,"n":1,"result":[{"said":"useful","failed":false}]});
        socket
            .write_all(&framed(reply.to_string().as_bytes()))
            .expect("reply");
        serde_json::from_slice(&body).expect("request json")
    });
    let mut call = call();
    call.args = json!({"command":"true", "event_context":{"turn_id":"forged-model-value"}});
    let answer = over_socket_context(&path, "private-key", &call, &Uncancelled, observed);
    let sent: Value = server.join().expect("joined server");
    assert_eq!(answer.expect("useful result").said, "useful");
    assert_eq!(sent["args"], json!([call]));
    assert_eq!(sent["key"], "private-key");
    assert_eq!(sent["call"], "run");
    sent
}

#[test]
fn only_negotiated_requests_capture_current_host_context() {
    if isolated() {
        return;
    }
    assert!(exchange(true).get("event_context").is_none());
    assert!(launch::install(|_, _, _| None, context));
    for (observed, active, queries) in [
        (false, true, 0),
        (true, true, 1),
        (true, false, 2),
        (false, true, 2),
        (true, true, 3),
    ] {
        ACTIVE.store(active, Ordering::SeqCst);
        let sent = exchange(observed);
        if observed && active {
            assert_eq!(
                sent["event_context"],
                json!({"schema":1,"run_id":ID,"agent_id":ID,"turn_id":ID,"ask_id":null})
            );
        } else {
            assert!(sent.get("event_context").is_none());
        }
        assert_eq!(QUERIES.load(Ordering::SeqCst), queries);
    }
}
