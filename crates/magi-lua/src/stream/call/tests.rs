//! `handle:call` against a socket the test answers by hand.

use super::{CAP, frame};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

/// Run `lua` with `h` connected to a socket whose one connection `serve` answers. Answers what the
/// script left in `magi.answer`, and what `serve` returned.
fn called<T: Send + 'static>(
    name: &str,
    serve: impl FnOnce(UnixStream) -> T + Send + 'static,
    lua: &str,
) -> (String, T) {
    let dir = magi_tools::family::FamilyScratch::new(name).expect("a family scratch");
    let path = dir.join("s.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    let server = std::thread::spawn(move || serve(listener.accept().expect("accept").0));
    let script = format!(
        "local h = assert(magi.stream.connect({:?}, 2000))\n{lua}",
        path.to_string_lossy()
    );
    let mut engine = crate::Engine::new();
    engine.run(&script, "call.lua").expect("the script runs");
    engine.harvest();
    let served = server.join().expect("the server");
    let config = engine.config();
    let answer = config.string("answer").expect("an answer").to_owned();
    (answer, served)
}

/// Read one call frame off `stream`, answering its body.
fn call_on(stream: &mut UnixStream) -> Vec<u8> {
    let mut head = [0_u8; 4];
    stream.read_exact(&mut head).expect("a header");
    let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
    stream.read_exact(&mut body).expect("a body");
    body
}

/// Every byte `stream` carries until its peer closes it.
fn rest_of(stream: &mut UnixStream) -> usize {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a deadline");
    let mut rest = Vec::new();
    stream
        .read_to_end(&mut rest)
        .map_or(usize::MAX, |_| rest.len())
}

#[test]
fn a_request_goes_out_as_one_json_frame_and_the_reply_comes_back_a_table() {
    let (answer, asked) = called(
        "round",
        |mut stream| {
            let body = call_on(&mut stream);
            let reply = br#"{"ok":true,"family":1,"n":1,"result":["x"]}"#;
            stream
                .write_all(&(reply.len() as u32).to_be_bytes())
                .expect("head");
            stream.write_all(reply).expect("body");
            serde_json::from_slice::<serde_json::Value>(&body).expect("json")
        },
        "local r, why = h:call({ call = 'verbs', args = { 1 } }, { deadline_ms = 2000 })\n\
         magi.answer = tostring(r.ok) .. '|' .. r.result[1] .. '|' .. tostring(why)",
    );
    assert_eq!(answer, "true|x|nil");
    assert_eq!(asked, serde_json::json!({ "call": "verbs", "args": [1] }));
}

#[test]
fn a_frame_holds_the_cap_and_not_a_byte_more() {
    assert!(frame(&vec![b'a'; CAP]).is_ok());
    let why = frame(&vec![b'a'; CAP + 1]).expect_err("over the cap");
    assert!(why.contains("not sent"), "{why}");
}

#[test]
fn a_call_one_byte_over_the_cap_is_refused_before_a_byte_is_sent() {
    // `{"call":"x","pad":""}` is 21 bytes before the padding.
    let (answer, arrived) = called(
        "over",
        |mut stream| rest_of(&mut stream),
        &format!(
            "local r, why = h:call({{ call = 'x', pad = string.rep('a', {}) }})\n\
             h:close()\n\
             magi.answer = tostring(r) .. '|' .. why",
            CAP + 1 - 21
        ),
    );
    let refused = format!("the call is {} bytes and a frame carries at most", CAP + 1);
    assert!(
        answer.starts_with("nil|") && answer.contains(&refused),
        "{answer}"
    );
    assert_eq!(arrived, 0, "bytes reached the peer");
}

#[test]
fn a_short_read_takes_the_connection_with_it() {
    let (answer, ()) = called(
        "short",
        |mut stream| {
            call_on(&mut stream);
            stream.write_all(&100_u32.to_be_bytes()).expect("head");
            stream.write_all(&[b'{'; 10]).expect("part of a body");
        },
        "local r, why = h:call({ call = 'verbs' })\n\
         local again, gone = h:call({ call = 'verbs' })\n\
         magi.answer = tostring(r) .. '|' .. why .. '|' .. tostring(again) .. '|' .. gone",
    );
    assert!(answer.contains("closed 10 bytes into 100"), "{answer}");
    assert!(
        answer.ends_with("|nil|the connection is closed"),
        "{answer}"
    );
}

#[test]
fn a_reply_over_the_cap_is_refused_before_its_body_is_read() {
    let (answer, after) = called(
        "reply",
        |mut stream| {
            call_on(&mut stream);
            let over = u32::try_from(CAP + 1).expect("fits");
            stream.write_all(&over.to_be_bytes()).expect("head");
            rest_of(&mut stream)
        },
        "local r, why = h:call({ call = 'verbs' })\nmagi.answer = tostring(r) .. '|' .. why",
    );
    assert!(
        answer.starts_with("nil|") && answer.contains("a frame carries 1 to"),
        "{answer}"
    );
    assert_eq!(
        after, 0,
        "the connection was closed and nothing more was sent"
    );
}

#[test]
fn a_reply_that_does_not_come_by_the_deadline_takes_the_connection_with_it() {
    let started = std::time::Instant::now();
    let (answer, ()) = called(
        "late",
        |mut stream| {
            call_on(&mut stream);
            rest_of(&mut stream);
        },
        "local r, why = h:call({ call = 'verbs' }, { deadline_ms = 200 })\n\
         magi.answer = tostring(r) .. '|' .. why",
    );
    assert!(answer.starts_with("nil|awaiting the reply"), "{answer}");
    assert!(started.elapsed() < Duration::from_secs(4));
}
