//! A call keeps its own answer, driven against a socket the test owns.
//!
//! The wire carries no request id, so a reply is matched to a call by position — see FAMILY.md.
//! magi's client dials per call and is in step by construction; this measures that rather than
//! reading it off the source, and fails if the client is ever changed to hold its handle.

use magi_lua::Engine;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the client waits for a reply, how long the first reply is withheld past that, and how
/// long the test waits for the listener to say it wrote one.
const PATIENCE: Duration = Duration::from_millis(150);
const WITHHELD: Duration = Duration::from_millis(400);
const DEADLINE: Duration = Duration::from_secs(10);

/// A client library, read from the tree at run time the way the product reads it.
fn source(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A socket path this VM is allowed to dial: `magi.stream` refuses anything outside the runtime
/// directories, and it stays short enough for `SUN_LEN`.
fn socket_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mg-ds-{tag}-{}.sock", std::process::id()))
}

/// The verb a request frame names, without a JSON parser.
fn verb_of(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let Some(rest) = text.split("\"call\":\"").nth(1) else {
        return "?".to_owned();
    };
    rest.split('"').next().unwrap_or("?").to_owned()
}

/// Answer every call with the name of the verb that asked, so a reply read by the wrong call names
/// the call it really belongs to. The first answer of the run is withheld past the caller's
/// patience; each one written is announced, so the test never has to guess when it landed.
fn answer(mut stream: UnixStream, first: &Mutex<bool>, wrote: &Sender<String>) {
    loop {
        let mut head = [0_u8; 4];
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
        if stream.read_exact(&mut body).is_err() {
            return;
        }
        let verb = verb_of(&body);
        {
            let mut pending = first.lock().expect("the flag");
            if *pending {
                *pending = false;
                std::thread::sleep(WITHHELD);
            }
        }
        let reply = format!("{{\"ok\":true,\"n\":1,\"result\":[\"{verb}\"]}}");
        let mut frame = (reply.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(reply.as_bytes());
        // Announced whether or not it landed: a caller that dials per call has already dropped the
        // connection this reply was owed to, and the test still has to know the moment passed.
        let landed = stream.write_all(&frame).and_then(|()| stream.flush());
        if wrote.send(verb).is_err() || landed.is_err() {
            return;
        }
    }
}

/// A listener serving each connection on its own thread, for as long as the test holds it, and a
/// channel naming every reply it has put on the wire.
fn listen(path: &std::path::Path) -> std::sync::mpsc::Receiver<String> {
    std::fs::remove_file(path).ok();
    let listener = UnixListener::bind(path).expect("bind");
    let (wrote, written) = channel();
    let first = Arc::new(Mutex::new(true));
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let (first, wrote) = (Arc::clone(&first), wrote.clone());
            std::thread::spawn(move || answer(stream, &first, &wrote));
        }
    });
    written
}

/// One VM holding a live client, so the test can put the two calls either side of an event.
struct Caller {
    engine: Engine,
}

impl Caller {
    /// Load the library and open the connection, in the VM the calls will be made from.
    fn new(client: &str, chunk: &str, path: &std::path::Path) -> Self {
        let path = path.to_string_lossy().into_owned();
        let script = format!(
            r#"
            local chunk = assert(load({client:?}, {chunk:?}))
            local lib = chunk(magi.stream)
            __session = assert(lib.connect({{ path = {path:?}, timeout_ms = {ms} }}))
            "#,
            ms = PATIENCE.as_millis(),
        );
        let mut engine = Engine::new();
        engine.run(&script, "connect.lua").expect("a connection");
        Self { engine }
    }

    /// One call, as `value|why`. Both stringified: what a caller was handed matters here, not its
    /// type.
    fn call(&mut self, verb: &str) -> String {
        let script = format!(
            r#"
            local value, why = __session:call({verb:?})
            magi.answer = tostring(value) .. "|" .. tostring(why)
            "#
        );
        self.engine.run(&script, "call.lua").expect("a call runs");
        self.engine.harvest();
        self.engine
            .config()
            .string("answer")
            .expect("an answer")
            .to_owned()
    }
}

/// Wait for the listener to say it has written the reply to `verb`.
fn until_written(written: &std::sync::mpsc::Receiver<String>, verb: &str) {
    let deadline = std::time::Instant::now() + DEADLINE;
    while std::time::Instant::now() < deadline {
        match written.recv_timeout(DEADLINE) {
            Ok(seen) if seen == verb => return,
            Ok(_) => continue,
            Err(e) => panic!("the listener never wrote {verb}: {e}"),
        }
    }
    panic!("the listener never wrote {verb}");
}

#[test]
fn a_reconnecting_client_is_in_step_on_the_next_call() {
    // magi's own client dials per call, so an abandoned reply dies with its connection. Driven the
    // same way, so the claim is measured rather than read off the source.
    let path = socket_path("magi");
    let written = listen(&path);
    let mut caller = Caller::new(&source("lua/magi.lua"), "magi.lua", &path);

    let first = caller.call("alpha");
    assert!(
        first.starts_with("nil|"),
        "the withheld first call must not answer: {first}"
    );

    until_written(&written, "alpha");
    let second = caller.call("beta");
    std::fs::remove_file(&path).ok();

    assert!(
        second.starts_with("beta|"),
        "the second call must get its own answer: {second}"
    );
}
