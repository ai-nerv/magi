//! `workbench` — a program that fills magi's `tools` role and nothing else.
//!
//! Written from ROLES.md alone, against no magi source. It answers the family floor (`verbs`,
//! `client`) and the tools core (`tools`, `run`), refuses both extensions by name in the reply
//! shape, and offers one tool, `backwards`, which reverses a piece of text. Every call it runs is
//! appended to `.workbench/calls` in the directory it was started in, beside the settings it was
//! handed, so a test can see that it — and not casper — was what ran.
//!
//! No dependencies: `rustc workbench.rs -O -o workbench`.

use std::io::{Read, Write};

const FAMILY: &str = "\"family\":1,\"surface\":1";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let verb = argv
        .iter()
        .map(String::as_str)
        .find(|a| !a.starts_with("--"))
        .unwrap_or("verbs");

    let reply = match verb {
        "verbs" => verbs(),
        "client" => refused("workbench lends no client library"),
        "tools" => ok(&[CARD.to_owned()]),
        "run" => run(),
        "surface" | "acknowledge" => refused(&format!("`{verb}` is an extension workbench does not implement")),
        other => refused(&format!("no such call: {other}")),
    };
    println!("{reply}");
}

/// The one tool on offer. It touches nothing, so it names no permission verb.
const CARD: &str = r#"{"name":"backwards","description":"Reverse a piece of text","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}"#;

/// Every verb this program answers. The extensions are listed and refused, never omitted.
fn verbs() -> String {
    let rows: Vec<String> = [
        ("verbs", "Every verb this program answers"),
        ("client", "The Lua library that speaks this surface"),
        ("tools", "Every tool on offer, with its schema"),
        ("run", "Run one tool; the call arrives as JSON on stdin"),
        ("surface", "Refused: workbench holds no rows"),
        ("acknowledge", "Refused: workbench installs nothing"),
    ]
    .iter()
    .map(|(verb, about)| format!(r#"{{"verb":"{verb}","about":"{about}","door":"cli"}}"#))
    .collect();
    ok(&rows)
}

/// One call: read it, reverse the text in it, and keep a line saying so.
fn run() -> String {
    let mut body = String::new();
    if std::io::stdin().read_to_string(&mut body).is_err() {
        return refused("the call could not be read");
    }
    let Some(text) = field(&body, "text") else {
        return refused("`backwards` needs a `text`");
    };
    let settings = std::env::var("MAGI_TOOLS_CONFIGURE").unwrap_or_default();
    let dir = std::path::Path::new(".workbench");
    std::fs::create_dir_all(dir).ok();
    if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("calls")) {
        writeln!(log, "{text}\t{settings}").ok();
    }
    let said = format!("workbench ran: {}", text.chars().rev().collect::<String>());
    ok(&[format!("{{\"said\":{}}}", quoted(&said))])
}

/// The string value of `"name":"…"` in a JSON body. Enough for one flat argument, and no more.
fn field(body: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\"");
    let after = &body[body.find(&key)? + key.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = after.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}

fn ok(values: &[String]) -> String {
    format!("{{\"ok\":true,{FAMILY},\"n\":{},\"result\":[{}]}}", values.len(), values.join(","))
}

fn refused(why: &str) -> String {
    format!("{{\"ok\":false,{FAMILY},\"n\":0,\"result\":[],\"fault\":\"refused\",\"error\":{}}}", quoted(why))
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
