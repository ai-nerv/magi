//! Talking to a sibling that is actually running.
//!
//! Every discovery bug is invisible from inside the tool that owns it: a socket in the wrong
//! directory, a lister that only sees its own host, a name that does not match its file — all
//! of them work when a tool talks to itself. These run only when a sibling is live, and skip
//! quietly otherwise, because a test that needs someone else's daemon must not fail a build on
//! a machine that has none.

use axon_lua::Engine;
use axon_lua::peer::{SIBLINGS, call};
use std::path::PathBuf;

/// A sibling's client, if its checkout is beside ours.
fn client_of(name: &str, relative: &str) -> Option<String> {
    // Anchored to this crate, not the working directory: cargo runs a test from the package
    // root, so `..` is `crates/` and every sibling lookup silently found nothing.
    let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    std::fs::read_to_string(tools.join(name).join(relative)).ok()
}

/// Whether that sibling has a socket to answer on.
fn is_live(name: &str) -> bool {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::read_dir(runtime.join(name)).is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".sock"))
    })
}

#[test]
fn a_live_sibling_answers_verbs() {
    let mut talked_to = 0;

    for sibling in SIBLINGS {
        let Some(client) = client_of(sibling.name, sibling.client) else {
            continue;
        };
        if !is_live(sibling.name) {
            continue;
        }

        let mut engine = Engine::new();
        let socket = axon_lua::peer::socket_of(sibling.name);
        let answer = call(
            &mut engine,
            &client,
            "verbs",
            socket.as_deref().map(|p| p.to_string_lossy()).as_deref(),
        )
        .expect("the call must run");

        // Not running is not failing. `is_live` looks for any socket under the sibling's
        // runtime directory; the client looks for the *particular* one it speaks to — `api@*` for
        // hexe, `onix/oslo/*` for oslo. A mux that has exited leaves its pane sockets behind, so
        // the two disagree, and treating that as a failure tests whose machine it ran on rather
        // than whether the code works.
        if answer.contains("socket found") {
            eprintln!(
                "{}: nothing listening for the client to talk to",
                sibling.name
            );
            continue;
        }

        // `verbs` ships from version one precisely so this question has an answer. A sibling
        // that connects but cannot say what it does is one axon cannot use safely.
        assert!(
            !answer.starts_with("no: "),
            "{} is live but would not connect: {answer}",
            sibling.name
        );
        assert!(
            !answer.is_empty() && answer != "no answer",
            "{} connected and said nothing",
            sibling.name
        );
        eprintln!("{} answered verbs: {answer}", sibling.name);
        talked_to += 1;
    }

    if talked_to == 0 {
        eprintln!("no sibling was running; nothing to talk to");
    }
}
