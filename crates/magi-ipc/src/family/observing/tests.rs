use super::*;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const ID: &str = "0123456789abcdef0123456789abcdef";
const CHILD: &str = "MAGI_MEMORY_CONTEXT_TEST";
static ACTIVE: AtomicBool = AtomicBool::new(true);

fn context() -> Option<Context> {
    ACTIVE
        .load(Ordering::SeqCst)
        .then(|| Context::new(ID.into(), ID.into(), Some(ID.into()), None).expect("context"))
}

struct Held(Option<Child>);

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn(name: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args(["--exact", &format!("family::observing::tests::{name}")])
        .env(CHILD, name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn joined(mut child: Held) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while child
        .0
        .as_mut()
        .expect("child")
        .try_wait()
        .expect("status")
        .is_none()
    {
        assert!(Instant::now() < deadline, "child deadline");
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
}

fn accept(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).expect("bounded accept");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((socket, _)) => {
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("read deadline");
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .expect("write deadline");
                return socket;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "accept deadline");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept: {error}"),
        }
    }
}

fn echo(socket: &mut UnixStream, revision: Value) -> Value {
    let mut header = [0; 4];
    socket.read_exact(&mut header).expect("header");
    let length = u32::from_be_bytes(header) as usize;
    assert!((1..=8192).contains(&length));
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes).expect("body");
    let request: Value = crate::Wire::read(&bytes).expect("request");
    let body = serde_json::to_vec(
        &json!({"ok":true,"family":1,"n":1,"result":[request],"request_context":revision}),
    )
    .expect("reply");
    socket
        .write_all(&(body.len() as u32).to_be_bytes())
        .expect("header");
    socket.write_all(&body).expect("body");
    request
}

#[test]
fn replacement_peer() {
    if std::env::var(CHILD).as_deref() != Ok("replacement_peer") {
        return;
    }
    let path = std::env::var_os("MAGI_MEMORY_CONTEXT_SOCKET").expect("path");
    let listener = UnixListener::bind(&path).expect("replacement");
    let request = echo(&mut accept(&listener), json!(1));
    assert!(request.get("event_context").is_none());
}

#[test]
fn native_clients_negotiate_on_the_actual_peer_and_keep_model_arguments_unchanged() {
    const NAME: &str =
        "native_clients_negotiate_on_the_actual_peer_and_keep_model_arguments_unchanged";
    if std::env::var(CHILD).as_deref() != Ok(NAME) {
        joined(Held(Some(spawn(NAME).spawn().expect("isolated"))));
        return;
    }
    let root = magi_testkit::Scratch::new("memory-context", "wire");
    let path = root.join("memory.sock");
    let listener = UnixListener::bind(&path).expect("listener");
    let (read, _) = UnixStream::pair().expect("pair");
    received(&read, &path, "verbs", &json!({"request_context":1}));
    assert!(OBSERVED.lock().expect("registry").is_empty());
    assert!(install(context));
    let server = std::thread::spawn(move || {
        for count in [5, 2] {
            let mut socket = accept(&listener);
            for index in 0..count {
                echo(&mut socket, if index == 3 { Value::Null } else { json!(1) });
            }
        }
    });
    let args = vec![json!({"event_context":{"turn_id":"forged-model-value"}})];
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut client = super::super::Family::dial(&path)
            .await
            .expect("dial")
            .speaking(crate::Wire::Cbor);
        let reply = client.call("verbs", vec![]).await.expect("verbs");
        assert!(reply[0].get("event_context").is_none());
        let reply = client.call("layout", args.clone()).await.expect("call");
        assert_eq!(
            reply[0]["event_context"],
            json!({"schema":1,"run_id":ID,"agent_id":ID,"turn_id":ID,"ask_id":null})
        );
        assert_eq!(reply[0]["args"], json!(args));
        ACTIVE.store(false, Ordering::SeqCst);
        assert!(
            client.call("layout", vec![]).await.expect("no context")[0]
                .get("event_context")
                .is_none()
        );
        ACTIVE.store(true, Ordering::SeqCst);
        client
            .call("verbs", vec![])
            .await
            .expect("capability withdrawn");
        assert!(
            client.call("layout", vec![]).await.expect("old peer")[0]
                .get("event_context")
                .is_none()
        );
    });
    let mut client = super::super::blocking::Family::dial(&path).expect("blocking dial");
    client.call("verbs", vec![]).expect("blocking verbs");
    let reply = client.call("layout", args.clone()).expect("blocking call");
    assert_eq!(reply[0]["args"], json!(args));
    assert_eq!(reply[0]["event_context"]["turn_id"], ID);
    drop(client);
    server.join().expect("responder");
    std::fs::remove_file(&path).expect("old socket");
    let child = Held(Some(
        spawn("replacement_peer")
            .env("MAGI_MEMORY_CONTEXT_SOCKET", &path)
            .spawn()
            .expect("replacement process"),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "replacement bind deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut client = super::super::blocking::Family::dial(&path).expect("replacement dial");
    assert!(
        client.call("layout", vec![]).expect("replacement call")[0]
            .get("event_context")
            .is_none()
    );
    drop(client);
    joined(child);
    for invalid in [Value::Null, json!("1"), json!(2), json!(true)] {
        received(&read, &path, "verbs", &json!({"request_context":invalid}));
        assert!(capture(&read, &path).is_none());
    }
    OBSERVED.lock().expect("registry").clear();
    for n in 0..(PEERS + 5) {
        received(
            &read,
            &PathBuf::from(format!("peer{n}")),
            "verbs",
            &json!({"request_context":1}),
        );
    }
    assert_eq!(OBSERVED.lock().expect("registry").len(), PEERS);
    assert!(capture(&read, Path::new("peer32")).is_none());
}
