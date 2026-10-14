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
fn a_frame_of_transcript_entries_each_way_fits_the_vm() {
    let entry = serde_json::json!({
        "cursor": 1, "role": "assistant", "text": "word ".repeat(40), "at": "2026-10-14T00:00:00.000Z",
    });
    let room = CAP - 4096;
    let count = room / (serde_json::to_vec(&entry).expect("encode").len() + 1);
    let reply = serde_json::json!({ "ok": true, "family": 1, "result": vec![entry; count] });
    let reply = serde_json::to_vec(&reply).expect("encode");
    let (answer, asked) = called(
        "dense",
        move |mut stream| {
            let body = call_on(&mut stream);
            let head = u32::try_from(reply.len()).expect("fits");
            stream.write_all(&head.to_be_bytes()).expect("head");
            stream.write_all(&reply).expect("body");
            body.len()
        },
        &format!(
            "local entries, at = {{}}, 0\n\
             while at < {room} do\n\
               entries[#entries + 1] = {{ cursor = #entries + 1, role = 'assistant',\n\
                 text = string.rep('word ', 40) .. #entries, at = '2026-10-14T00:00:00.000Z' }}\n\
               at = at + 290\n\
             end\n\
             local request = {{ call = 'remember', args = {{ 'session', {{ entries = entries }} }} }}\n\
             local r, why = h:call(request, {{ deadline_ms = 20000 }})\n\
             magi.answer = tostring(r and #r.result) .. '|' .. tostring(why)"
        ),
    );
    assert_eq!(answer, format!("{count}|nil"));
    assert!(asked > CAP - (CAP >> 3), "the call carried {asked} bytes");
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

/// balthasar's own client library, from a balthasar checkout beside this one, when it frames
/// through `handle:call`.
fn balthasar_library() -> Option<String> {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|at| at.join("balthasar/crates/balthasar-lua/lua/balthasar.lua"))
        .find_map(|path| std::fs::read_to_string(path).ok())
        .filter(|library| library.contains("handle:call("))
}

/// A memory layer on `path` that answers `verbs` to anybody and `remember` only with `key`, on
/// one connection, until it closes. Answers each call's verb and the key it carried.
fn keyed_serve(
    path: &std::path::Path,
    key: String,
) -> std::thread::JoinHandle<Vec<(String, Option<String>)>> {
    let listener = UnixListener::bind(path).expect("bind");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut heard = Vec::new();
        let mut head = [0_u8; 4];
        while stream.read_exact(&mut head).is_ok() {
            let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
            stream.read_exact(&mut body).expect("a body");
            let call: serde_json::Value = serde_json::from_slice(&body).expect("json");
            let verb = call["call"].as_str().unwrap_or_default().to_owned();
            let carried = call["key"].as_str().map(str::to_owned);
            let reply = match verb.as_str() {
                "remember" if carried.as_deref() != Some(key.as_str()) => serde_json::json!({
                    "ok": false, "family": 1, "fault": "refused",
                    "error": "a call to serve needs the key it was started with",
                }),
                "remember" => serde_json::json!({
                    "ok": true, "family": 1, "n": 1, "result": [{ "landing": "scratch", "id": "m1" }],
                }),
                _ => {
                    serde_json::json!({ "ok": true, "family": 1, "n": 1, "result": [{ "verb": "remember" }] })
                }
            };
            let reply = serde_json::to_vec(&reply).expect("encode");
            stream
                .write_all(&(reply.len() as u32).to_be_bytes())
                .expect("head");
            stream.write_all(&reply).expect("body");
            heard.push((verb, carried));
        }
        heard
    })
}

/// What balthasar's library made of a `remember` on `path`, as `id|why`.
fn remembered_through(library: &str, path: &std::path::Path) -> String {
    let script = format!(
        "local lib = assert(load({library:?}, 'balthasar.lua'))(magi.stream)\n\
         local mem = assert(lib.connect({{ path = {:?}, timeout_ms = 2000 }}))\n\
         local kept, why = mem.remember('the build is make')\n\
         mem:close()\n\
         magi.answer = tostring(kept and kept.id) .. '|' .. tostring(why)",
        path.to_string_lossy()
    );
    let mut engine = crate::Engine::new();
    engine
        .run(&script, "remember.lua")
        .expect("the script runs");
    engine.harvest();
    let config = engine.config();
    config.string("answer").expect("an answer").to_owned()
}

#[test]
fn balthasars_library_writes_to_a_keyed_serve_through_call_and_never_holds_the_key() {
    let Some(library) = balthasar_library() else {
        magi_testkit::live::unavailable("no balthasar checkout whose library frames through call");
        return;
    };
    let dir = magi_tools::family::FamilyScratch::new("keyed-call").expect("a family scratch");
    let key = "cd".repeat(32);
    let (keyed, bare) = (dir.join("api@keyed.sock"), dir.join("api@bare.sock"));
    magi_tools::family::hold_key(&keyed, &key);

    let serving = keyed_serve(&keyed, key.clone());
    assert_eq!(remembered_through(&library, &keyed), "m1|nil");
    let heard = serving.join().expect("the keyed serve");
    assert!(
        heard.contains(&("remember".to_owned(), Some(key.clone()))),
        "{heard:?}"
    );

    let serving = keyed_serve(&bare, key);
    let refused = remembered_through(&library, &bare);
    assert!(
        refused.starts_with("nil|a call to serve needs the key"),
        "{refused}"
    );
    let heard = serving.join().expect("the bare serve");
    assert!(
        heard.iter().all(|(_, carried)| carried.is_none()),
        "{heard:?}"
    );
}
