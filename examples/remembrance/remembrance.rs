//! `remembrance` — a program that fills magi's `memory` role and nothing else.
//!
//! Written from ROLES.md alone, against no magi source. It answers the family floor (`verbs`,
//! `client`) and the memory core (`observe`, `replay`), refuses every extension by name in the
//! reply shape, and keeps what it is told as one JSON object per line under `.remembrance/` in the
//! project it was started in.
//!
//! No dependencies: `rustc remembrance.rs -O -o remembrance`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const FAMILY: &str = "\"family\":1,\"surface\":1";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = argv
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json" && *a != "--cbor")
        .collect();
    let cbor = argv.iter().any(|a| a == "--cbor");
    let verb = words.first().copied().unwrap_or("verbs");

    match verb {
        "verbs" if cbor => {
            // One CBOR map, `{"ok": true}`: enough for a caller to see the encoding is answered.
            std::io::stdout().write_all(&[0xA1, 0x62, 0x6F, 0x6B, 0xF5]).ok();
        }
        "verbs" => println!("{}", verbs()),
        "client" => println!("{}", refused("remembrance lends no client library: it fills the memory core and declares no tools")),
        "serve" => serve(&words),
        "observe" | "replay" | "sessions" => {
            println!("{}", refused(&format!("`{verb}` is answered on the socket; run `remembrance serve`")))
        }
        other => println!("{}", refused(&format!("no such call: {other}"))),
    }
}

/// Every verb this program answers, on each of its doors.
fn verbs() -> String {
    let rows = [
        r#"{"verb":"verbs","about":"Every verb this program answers","door":"cli"}"#,
        r#"{"verb":"client","about":"The Lua library that speaks this surface","door":"cli"}"#,
        r#"{"verb":"serve","about":"Listen for other programs","door":"cli"}"#,
        r#"{"verb":"verbs","name":"verbs","about":"every name this remembrance will answer","writes":false,"door":"socket"}"#,
        r#"{"verb":"client","name":"client","about":"the Lua library that speaks this surface, as source","writes":false,"door":"socket"}"#,
        r#"{"verb":"observe","name":"observe","about":"stream a turn as it settles: (session, turn) -> ok","writes":true,"door":"socket"}"#,
        r#"{"verb":"replay","name":"replay","about":"everything a run said, in order: (session) -> [turn]","writes":false,"door":"socket"}"#,
        r#"{"verb":"sessions","name":"sessions","about":"the runs this project has had","writes":false,"door":"socket"}"#,
    ];
    format!(
        "{{\"ok\":true,{FAMILY},\"n\":{},\"result\":[{}]}}",
        rows.len(),
        rows.join(",")
    )
}

fn ok(values: &[String]) -> String {
    format!(
        "{{\"ok\":true,{FAMILY},\"n\":{},\"result\":[{}]}}",
        values.len(),
        values.join(",")
    )
}

fn refused(why: &str) -> String {
    format!(
        "{{\"ok\":false,{FAMILY},\"n\":0,\"result\":[],\"fault\":\"refused\",\"error\":{}}}",
        quoted(why)
    )
}

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---- the socket ---------------------------------------------------------------------------

/// Bind where magi looks, answer until the process it is tied to goes.
fn serve(words: &[&str]) {
    let flag = |name: &str| {
        words
            .iter()
            .position(|w| *w == name)
            .and_then(|at| words.get(at + 1))
            .map(|s| (*s).to_owned())
    };
    let instance = flag("--instance").unwrap_or_else(|| "one".to_owned());
    let dir = socket_dir();
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("api@{instance}.sock"));
    let _ = std::fs::remove_file(&path);

    let store = store_dir();
    std::fs::create_dir_all(&store).ok();

    if let Some(tied) = flag("--tied").and_then(|p| p.parse::<u32>().ok()) {
        std::thread::spawn(move || {
            while Path::new(&format!("/proc/{tied}")).exists() {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            std::process::exit(0);
        });
    }

    let listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(why) => {
            eprintln!("remembrance: cannot bind {}: {why}", path.display());
            std::process::exit(1);
        }
    };
    for stream in listener.incoming().flatten() {
        let store = store.clone();
        std::thread::spawn(move || held(stream, &store));
    }
}

/// One connection, many calls: read a frame, answer it, read the next.
fn held(mut stream: std::os::unix::net::UnixStream, store: &Path) {
    loop {
        let mut head = [0_u8; 4];
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        let len = u32::from_be_bytes(head) as usize;
        let mut body = vec![0_u8; len];
        if stream.read_exact(&mut body).is_err() {
            return;
        }
        let reply = answer(&String::from_utf8_lossy(&body), store);
        let bytes = reply.as_bytes();
        let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(bytes);
        if stream.write_all(&frame).is_err() {
            return;
        }
    }
}

/// What one call is answered with.
fn answer(body: &str, store: &Path) -> String {
    let Some(call) = field(body, "call") else {
        return refused("a call names a verb");
    };
    let args = arguments(body);
    match call.as_str() {
        "verbs" => verbs(),
        "client" => refused(
            "remembrance lends no client library: it fills the memory core and declares no tools",
        ),
        "observe" => match (args.first(), args.get(1)) {
            (Some(session), Some(turn)) => observe(store, &unquote(session), turn),
            _ => refused("observe takes (session, turn)"),
        },
        "replay" => match args.first() {
            Some(session) => ok(&replay(store, &unquote(session))),
            None => refused("replay takes (session)"),
        },
        "sessions" => ok(&sessions(store)),
        other => refused(&format!(
            "`{other}` is not one this remembrance answers; it fills the memory core only"
        )),
    }
}

/// Keep one turn. Appended, and read back last-write-wins per cursor, so a second write at a
/// cursor already held is the amend ROLES.md says an implementation may make of it.
fn observe(store: &Path, session: &str, turn: &str) -> String {
    let path = store.join(format!("{}.jsonl", safe(session)));
    let mut line = turn.replace('\n', " ");
    line.push('\n');
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()).and_then(|()| file.sync_all()))
    {
        Ok(()) => ok(&[]),
        Err(why) => format!(
            "{{\"ok\":false,{FAMILY},\"n\":0,\"result\":[],\"fault\":\"failed\",\"error\":{}}}",
            quoted(&format!("{}: {why}", path.display()))
        ),
    }
}

/// Everything a run said, in cursor order, as it finally stood.
fn replay(store: &Path, session: &str) -> Vec<String> {
    let path = store.join(format!("{}.jsonl", safe(session)));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut held: Vec<(u64, String)> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let cursor = number(line, "cursor").unwrap_or(u64::MAX);
        match held.iter_mut().find(|(at, _)| *at == cursor) {
            Some(row) => row.1 = line.to_owned(),
            None => held.push((cursor, line.to_owned())),
        }
    }
    held.sort_by_key(|(at, _)| *at);
    held.into_iter().map(|(_, line)| line).collect()
}

/// The runs this project has had, newest first.
fn sessions(store: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(store) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let when = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let rows = replay(store, id);
        let title = rows
            .first()
            .and_then(|row| field(row, "text"))
            .unwrap_or_default();
        found.push((
            when,
            format!(
                "{{\"id\":{},\"title\":{},\"entries\":{}}}",
                quoted(id),
                quoted(title.chars().take(80).collect::<String>().as_str()),
                rows.len()
            ),
        ));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, row)| row).collect()
}

// ---- enough JSON to read a call -------------------------------------------------------------

/// The values of `args`, each as the text it was written as.
fn arguments(body: &str) -> Vec<String> {
    let Some(at) = body.find("\"args\":") else {
        return Vec::new();
    };
    let bytes = body.as_bytes();
    let mut i = at + "\"args\":".len();
    while i < bytes.len() && bytes[i] != b'[' {
        i += 1;
    }
    i += 1;
    let mut out = Vec::new();
    loop {
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b']' {
            return out;
        }
        let end = value_end(bytes, i);
        out.push(body[i..end].to_owned());
        i = end;
        while i < bytes.len() && (bytes[i] == b',' || (bytes[i] as char).is_whitespace()) {
            i += 1;
        }
    }
}

/// Where the JSON value starting at `from` ends.
fn value_end(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    match bytes.get(i) {
        Some(b'"') => {
            i += 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => i += 2,
                    b'"' => return i + 1,
                    _ => i += 1,
                }
            }
            i
        }
        Some(b'{') | Some(b'[') => {
            let mut depth = 0_i32;
            let mut quoted = false;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if quoted => i += 1,
                    b'"' => quoted = !quoted,
                    b'{' | b'[' if !quoted => depth += 1,
                    b'}' | b']' if !quoted => {
                        depth -= 1;
                        if depth == 0 {
                            return i + 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            i
        }
        _ => {
            while i < bytes.len() && bytes[i] != b',' && bytes[i] != b']' && bytes[i] != b'}' {
                i += 1;
            }
            i
        }
    }
}

/// One top-level-ish string field's value, unescaped enough to be read back.
fn field(text: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":");
    let at = text.find(&key)? + key.len();
    let rest = text.get(at..)?.trim_start();
    rest.starts_with('"').then(|| unquote(rest))
}

/// One number field's value.
fn number(text: &str, name: &str) -> Option<u64> {
    let key = format!("\"{name}\":");
    let at = text.find(&key)? + key.len();
    let rest = text.get(at..)?.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A JSON string as its text.
fn unquote(text: &str) -> String {
    let text = text.trim();
    if !text.starts_with('"') {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut chars = text[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    out
}

/// A session id as a filename.
fn safe(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Where magi looks for this session's socket. `balthasar` is in the path because the family wire
/// still names the program rather than the role — see PLAN-SWAPPABLE.md's M3.
fn socket_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join("balthasar"),
        None => std::env::temp_dir().join("balthasar"),
    }
}

/// Where the turns are kept: the project this was started in, so a later run finds them.
fn store_dir() -> PathBuf {
    std::env::var_os("REMEMBRANCE_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join(".remembrance"))
}
