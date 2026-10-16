//! A block left running, for real: casper's own `screen`, through the holder the session uses, with
//! a real program on a real pty. Needs casper on `PATH` with its declarations where it reads them.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::holder::{DETACHED, Drawing, Holder, Holding};
use magi_proto::tooling::Shown;
use magi_proto::{HarnessEvent, ToolCallId};
use magi_tools::holding::{Context, Holds};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SAID: &str = "hello from the block";

/// What casper's `screen` asks for when called with `args`, or why it cannot be asked.
fn asked(args: &serde_json::Value) -> Result<magi_proto::tooling::Surface, String> {
    let call = serde_json::json!({ "tool": "screen", "args": args, "cwd": "/" });
    let mut child = std::process::Command::new("casper")
        .arg("run")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|why| format!("no casper on PATH: {why}"))?;
    std::io::Write::write_all(
        child.stdin.as_mut().ok_or("no stdin")?,
        call.to_string().as_bytes(),
    )
    .map_err(|why| why.to_string())?;
    drop(child.stdin.take());
    let out = child.wait_with_output().map_err(|why| why.to_string())?;
    let reply: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|why| format!("casper said: {why}"))?;
    match serde_json::from_value::<Shown>(reply["result"][0]["shown"].clone()) {
        Ok(Shown::Surface(surface)) => Ok(surface),
        _ => Err(format!("this casper's screen asks for no surface: {reply}")),
    }
}

/// Wait up to `most` for `seen` to hold an event `wanted` answers yes for.
fn waited(
    seen: &Mutex<Vec<HarnessEvent>>,
    most: Duration,
    wanted: impl Fn(&HarnessEvent) -> bool,
) -> bool {
    let began = Instant::now();
    while began.elapsed() < most {
        if seen.lock().expect("nothing panicked").iter().any(&wanted) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn screen_answers_at_once_and_its_program_draws_until_it_is_closed() {
    let args = serde_json::json!({
        "command": format!("printf '{SAID}\\n'; sleep 60"),
        "title": "probe",
        "rows": 4,
    });
    let surface = match asked(&args) {
        Ok(surface) if surface.detach => surface,
        Ok(_) => {
            magi_testkit::live::unavailable("this casper's screen does not run detached");
            return;
        }
        Err(why) => {
            magi_testkit::live::unavailable(&why);
            return;
        }
    };

    let held = Arc::new(Holding::new());
    let _screen = Drawing::attach(&held, true);
    held.sized(Some(20), 100, false);
    held.blocked(20, 94);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    let holder = Holder::new(
        Arc::clone(&held),
        Box::new(move |event| kept.lock().expect("nothing panicked").push(event)),
        Box::new(|| true),
        "casper",
    );
    let context = Context {
        call: Some(ToolCallId::new("call-1")),
        ..Context::default()
    };

    let began = Instant::now();
    let answered = holder.hold("screen", &surface, &args, &context);
    assert_eq!(answered.as_deref(), Some(DETACHED));
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "the call waited on its program"
    );

    let id = seen
        .lock()
        .expect("nothing panicked")
        .iter()
        .find_map(|event| match event {
            HarnessEvent::Surfaced {
                id, call, detach, ..
            } => {
                assert_eq!(call.as_ref(), Some(&ToolCallId::new("call-1")));
                assert!(detach);
                Some(id.clone())
            }
            _ => None,
        })
        .expect("its rows were reserved");
    let drew = waited(&seen, Duration::from_secs(15), |event| {
        matches!(event, HarnessEvent::Drew { id: drew, lines, .. }
            if *drew == id && lines.iter().flatten().any(|span| span.text.contains(SAID)))
    });
    assert!(drew, "its program never drew in its block");
    assert!(held.is_open(&id), "still running after its call answered");
    assert!(
        held.on_show()
            .iter()
            .any(|event| matches!(event, HarnessEvent::Drew { .. })),
        "a screen arriving now would be told its last frame"
    );

    held.close(&id);
    let closed = waited(
        &seen,
        Duration::from_secs(15),
        |event| matches!(event, HarnessEvent::Unsurfaced { id: gone, .. } if *gone == id),
    );
    assert!(closed, "closing it did not give its rows back");
}
