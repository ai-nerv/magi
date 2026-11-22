use super::*;
use magi_proto::{Cursor, MessageId};
use serde_json::{Value, json};

fn user(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(text),
        text: text.into(),
        aside: String::new(),
        images: vec![],
    }
}

fn key(ask: &Value) -> &str {
    let key = ask["wants"]["session_id"].as_str().expect("identity");
    assert_eq!(key.len(), 64);
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
    key
}

#[tokio::test]
async fn replacement_roles_receive_no_new_field_unless_their_catalog_advertises_it() {
    for capability in [None, Some(false), Some(true)] {
        let (session, _dir) = session("session-negotiation");
        session.lock().await.commit(user("hello")).expect("user");
        let mind = Mind::answering("session-negotiation", "answer");
        let mut catalog = magi_host::catalog::Catalog::empty();
        catalog.cwd = std::env::temp_dir();
        catalog.mind = mind.program().display().to_string();
        let mut card: magi_proto::ask::Card = serde_json::from_value(json!({"id":"fake/one","provider":"fake","name":"One","api":"fake","ready":true,"context_window":8192,"max_output":128,"capabilities":{"input":["text"],"output":["text"]}})).expect("card");
        card.capabilities.as_mut().expect("caps").session_identity = capability;
        catalog.cards.push(card);
        let mut backend = catalog.backend("fake/one").expect("backend");
        backend.confine = false;
        backend.isolate = false;
        backend.wants.session_id = Some("caller-forged-session".into());
        turn(&session, &backend).await;
        let asks = mind.asks();
        assert_eq!(asks.len(), 1);
        let actual: Value = serde_json::from_str(&asks[0]).expect("ask");
        if capability == Some(true) {
            assert_ne!(key(&actual), "caller-forged-session");
        } else {
            assert!(actual["wants"].get("session_id").is_none());
        }
    }
}

#[tokio::test]
async fn session_fixture_child() {
    let Some(root) = std::env::var_os("MAGI_SESSION_FIXTURE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let rows_path = root.join("rows.json");
    let rows: Vec<(Cursor, Entry)> =
        serde_json::from_slice(&std::fs::read(&rows_path).expect("rows")).expect("rows");
    let mut restored = Session::restored(SessionId::new("private-run"), rows).expect("restore");
    restored
        .commit(user(if restored.entries().is_empty() {
            "first-user"
        } else {
            "later-user"
        }))
        .expect("user");
    let session = tokio::sync::Mutex::new(restored);
    let mind = Mind::answering("session-restarted-host", "saved-answer");
    let mut backend = backend(&mind);
    backend.cwd = root.clone();
    backend.session_identity = true;
    backend.transcript = Some("private-run@agent".into());
    turn(&session, &backend).await;
    let asks = mind.asks();
    assert_eq!(asks.len(), 1);
    std::fs::write(root.join("ask.json"), &asks[0]).expect("capture");
    let held = session.lock().await;
    let rows: Vec<_> = held
        .entries()
        .iter()
        .enumerate()
        .map(|(i, entry)| (held.cursor_at(i).expect("cursor"), entry))
        .collect();
    std::fs::write(rows_path, serde_json::to_vec(&rows).expect("rows")).expect("persist fixture");
}

#[tokio::test]
async fn fresh_host_processes_restore_nonempty_history_without_changing_identity() {
    let root = Scratch::new("magi-turn", "session-processes");
    std::fs::write(root.join("rows.json"), b"[]").expect("empty history");
    let mut first = None;
    for round in 0..2 {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "session_identity::session_fixture_child",
                    "--nocapture",
                ])
                .env("MAGI_SESSION_FIXTURE_ROOT", root.as_os_str())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("bounded child")
        .expect("child");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        let ask: Value =
            serde_json::from_slice(&std::fs::read(root.join("ask.json")).expect("capture"))
                .expect("ask");
        if let Some(first) = &first {
            assert_eq!(key(&ask), first);
            let messages = ask["context"]["messages"].as_array().expect("history");
            assert_eq!(messages.len(), 3);
            for text in ["first-user", "saved-answer", "later-user"] {
                assert!(messages.iter().any(|m| m.to_string().contains(text)));
            }
        } else {
            first = Some(key(&ask).to_owned());
        }
        let rows: Vec<(Cursor, Entry)> =
            serde_json::from_slice(&std::fs::read(root.join("rows.json")).expect("rows"))
                .expect("rows");
        assert_eq!(rows.len(), (round + 1) * 2);
    }
}

#[test]
fn wire_gate_accepts_digest_input_but_still_refuses_uncapped_frame_lengths() {
    let root = Scratch::new("magi-turn", "session-wire-gate");
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for file in [
        "magi-ipc/src/codec.rs",
        "magi-ipc/src/runtime.rs",
        "magi-lua/lua/magi.lua",
        "magi-lua/src/stream/call.rs",
    ] {
        let destination = root.join(file);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("directory");
        std::fs::copy(workspace.join("crates").join(file), destination).expect("source fixture");
    }
    let module = root.join("magi-host/src/session_identity.rs");
    std::fs::create_dir_all(module.parent().expect("parent")).expect("directory");
    let hash = "sha2::Digest::update(&mut hash, (field.len() as u64).to_be_bytes());";
    let write = "wire.write_all(&(length as u32).to_be_bytes());";
    for (source, passes) in [
        (hash.to_owned(), true),
        (write.to_owned(), false),
        (format!("{hash}\n{write}"), false),
        (format!("{hash} {write}"), false),
        (
            format!("if length > magi_ipc::MAX_FRAME_BYTES {{ return; }}\n{write}"),
            true,
        ),
        ("let length = u32::from_be_bytes(prefix);".into(), false),
    ] {
        std::fs::write(&module, source).expect("fixture");
        let output = std::process::Command::new(workspace.join("scripts/gate-wire.sh"))
            .current_dir(&workspace)
            .env("GATE_ROOT", root.as_os_str())
            .output()
            .expect("gate");
        assert_eq!(
            output.status.success(),
            passes,
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !passes {
            assert!(String::from_utf8_lossy(&output.stderr).contains("a length"));
        }
    }
}
