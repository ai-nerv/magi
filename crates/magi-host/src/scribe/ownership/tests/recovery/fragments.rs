use super::*;
use magi_ipc::family::{DURABLE, MAX_FRAME_BYTES};

pub(super) struct Pages {
    width: usize,
    raw: Option<String>,
    damage: &'static str,
}

impl Pages {
    fn new() -> Self {
        Self {
            width: 19,
            raw: None,
            damage: "",
        }
    }

    pub(super) fn reply(&self, records: &BTreeMap<u64, Value>, request: &Value) -> Reply {
        let at = request["args"][1]["from"].as_u64().expect("from");
        let Some((cursor, record)) = records.range(at..).next() else {
            return Reply::rows(Vec::new());
        };
        let raw = self
            .raw
            .clone()
            .unwrap_or_else(|| record["raw"].to_string());
        let Some(offset) = request["args"][1].get("raw") else {
            let mut row = json!({"cursor":cursor,"truncated":true,"raw_bytes":raw.len()});
            match self.damage {
                "huge" => row["raw_bytes"] = json!(MAX_FRAME_BYTES + 1),
                "no-size" => {
                    row.as_object_mut().expect("object").remove("raw_bytes");
                }
                "zero-size" => row["raw_bytes"] = json!(0),
                "bad-size" => row["raw_bytes"] = json!("no"),
                "zero-cursor" => row["cursor"] = json!(0),
                _ => {}
            }
            return Reply::rows(vec![row]);
        };
        let offset = offset.as_u64().expect("offset") as usize;
        let end = raw.floor_char_boundary((offset + self.width).min(raw.len()));
        let mut row =
            json!({"cursor":cursor,"offset":offset,"raw_bytes":raw.len(),"raw":&raw[offset..end]});
        if end < raw.len() {
            row["next"] = json!(end);
        }
        match self.damage {
            "cursor" => row["cursor"] = json!(cursor + 1),
            "offset" => row["offset"] = json!(offset + 1),
            "size" => row["raw_bytes"] = json!(raw.len() + 1),
            "empty" => row["raw"] = json!(""),
            "overrun" => row["raw"] = json!("x".repeat(raw.len() + 1)),
            "next" => row["next"] = json!(offset),
            "early" => {
                row.as_object_mut().expect("object").remove("next");
            }
            "terminal" if end == raw.len() => row["next"] = Value::Null,
            "extra" => return Reply::rows(vec![row.clone(), row]),
            "no-row" => return Reply::rows(Vec::new()),
            _ => {}
        }
        Reply::rows(vec![row])
    }
}

async fn ready() -> (Fake, Arc<Mutex<Memory>>, Scribe) {
    let (fake, state, mut scribe) = memory().await;
    scribe
        .settle(Cursor(1), &user("before é 🐈"), &Beside::default())
        .await
        .expect("seed");
    lose(&mut scribe, &state, 2, false).await;
    state.lock().expect("memory").fragments = Some(Pages::new());
    (fake, state, scribe)
}

fn raw_calls(fake: &Fake) -> Vec<(usize, Value)> {
    fake.calls()
        .into_iter()
        .filter(|(_, call)| call["call"] == "replay" && call["args"][1].get("raw").is_some())
        .collect()
}

#[tokio::test]
async fn fragments_restore_full_records_for_recovery_and_loading() {
    let (fake, _state, mut scribe) = ready().await;
    scribe.recover_writer().await.expect("full recovery");
    assert!(raw_calls(&fake).len() > 1);
    assert_eq!(
        scribe.replay().await.expect("full replay"),
        vec![(Cursor(1), user("before é 🐈"))]
    );
    let witness = scribe.sent.clone();
    drop(scribe);
    let (mut loaded, entries) = fake
        .scribe("unused")
        .await
        .load_for("A", true)
        .await
        .expect("load");
    assert_eq!(entries, vec![(Cursor(1), user("before é 🐈"))]);
    assert_eq!(loaded.sent, witness);
    loaded.family = None;
    loaded
        .recover_writer()
        .await
        .expect("loaded fingerprint remains complete");
    assert_eq!(loaded.sent, witness);
}

#[tokio::test]
async fn unowned_and_other_transcript_reads_never_assemble_unstable_fragments() {
    let (fake, _state, scribe) = ready().await;
    drop(scribe);
    let mut inspection = fake.scribe("A").await;
    let preview = inspection.replay().await.expect("marked preview");
    assert_eq!(preview.len(), 1);
    assert!(
        matches!(&preview.first().expect("preview").1, Entry::Notice { text }
        if text.contains(crate::scribe::shape::CUT_SHORT))
    );
    assert!(raw_calls(&fake).is_empty());
    assert!(
        inspection
            .replay_record(
                "A",
                &json!({"cursor":1,"raw_bytes":64,"truncated":true}),
                true
            )
            .await
            .is_err()
    );
    let mut other = fake.scribe("B").await;
    other.reserve_writer(true).await.expect("own B");
    assert_eq!(other.replay_of("A").await.expect("read A"), preview);
    assert!(raw_calls(&fake).is_empty());
}

#[tokio::test]
async fn invalid_fragment_streams_preserve_the_writer_witness_and_pending_queue() {
    for damage in [
        "huge",
        "no-size",
        "zero-size",
        "bad-size",
        "zero-cursor",
        "cursor",
        "offset",
        "size",
        "empty",
        "overrun",
        "next",
        "early",
        "terminal",
        "extra",
        "no-row",
        "syntax",
        "embedded-cursor",
    ] {
        let (fake, state, scribe) = ready().await;
        {
            let mut state = state.lock().expect("memory");
            let mut pages = Pages::new();
            pages.damage = damage;
            if damage == "syntax" {
                pages.raw = Some("not json".into());
            }
            if damage == "embedded-cursor" {
                let mut raw = state.records[&1]["raw"].clone();
                raw["cursor"] = json!(9);
                pages.raw = Some(raw.to_string());
            }
            state.fragments = Some(pages);
        }
        let witness = scribe.sent.clone();
        let uncertain = scribe.uncertain;
        let session = tokio::sync::Mutex::new(
            crate::session::Session::restored(
                SessionId::new("A"),
                vec![(Cursor(1), user("before é 🐈"))],
            )
            .expect("session"),
        );
        session.lock().await.commit(user("queued")).expect("queue");
        let pending = session.lock().await.pending_batch();
        let mut held = Some(scribe);
        assert!(
            flush(&session, &mut held)
                .await
                .expect_err(damage)
                .is_fatal()
        );
        assert_eq!(session.lock().await.pending_batch(), pending);
        let scribe = held.expect("scribe");
        assert_eq!(scribe.sent, witness);
        assert_eq!(scribe.uncertain, uncertain);
        assert!(scribe.check_writer().is_err());
        assert!(
            fake.calls()
                .iter()
                .filter(|(connection, _)| *connection > 1)
                .all(|(_, call)| call["call"] != "observe" && call["call"] != "amend")
        );
        if ["huge", "no-size", "zero-size", "bad-size", "zero-cursor"].contains(&damage) {
            assert!(raw_calls(&fake).is_empty(), "{damage}");
        }
    }
}

#[tokio::test]
async fn fragment_count_is_exactly_bounded_without_accepting_partial_history() {
    for total in [256, 257] {
        let (fake, state, mut scribe) = ready().await;
        let raw = state.lock().expect("memory").records[&1]["raw"]
            .to_string()
            .replace('é', "\\u00e9")
            .replace('🐈', "\\ud83d\\udc08");
        let mut pages = Pages::new();
        pages.width = 1;
        pages.raw = Some(format!("{}{}", " ".repeat(total - raw.len()), raw));
        state.lock().expect("memory").fragments = Some(pages);
        let result = scribe.recover_writer().await;
        assert_eq!(result.is_ok(), total == 256, "{result:?}");
        assert_eq!(raw_calls(&fake).len(), 256);
        assert_eq!(scribe.uncertain.is_none(), total == 256);
    }
}

#[tokio::test]
async fn cancelled_fragments_drop_the_candidate_and_restart_at_zero() {
    let pause = Arc::new(Pause {
        verb: "replay",
        raw: true,
        armed: AtomicBool::new(false),
        arrived: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let state = Arc::new(Mutex::new(Memory::default()));
    let stored = Arc::clone(&state);
    let fake = Fake::paused(
        "fragment-cancel",
        move |call| stored.lock().expect("memory").answer(call),
        Some(Arc::clone(&pause)),
    )
    .await;
    let mut scribe = fake.scribe("A").await;
    scribe.reserve_writer(true).await.expect("reserve");
    scribe
        .settle(Cursor(1), &user("before"), &Beside::default())
        .await
        .expect("seed");
    lose(&mut scribe, &state, 2, false).await;
    state.lock().expect("memory").fragments = Some(Pages::new());
    let witness = scribe.sent.clone();
    let uncertain = scribe.uncertain;
    pause.armed.store(true, Ordering::SeqCst);
    let mut recovering = Box::pin(scribe.recover_writer());
    tokio::select! {
        result = &mut recovering => panic!("premature completion: {result:?}"),
        arrived = tokio::time::timeout(std::time::Duration::from_secs(2), pause.arrived.notified()) => arrived.expect("fragment read"),
    }
    drop(recovering);
    pause.release.notify_one();
    assert_eq!(scribe.sent, witness);
    assert_eq!(scribe.uncertain, uncertain);
    assert!(scribe.check_writer().is_err());
    scribe.retry.0 = std::time::Instant::now();
    scribe.recover_writer().await.expect("fresh bounded read");
    let calls = raw_calls(&fake);
    let first = calls
        .iter()
        .find(|(connection, _)| *connection == 3)
        .expect("fresh connection");
    assert_eq!(first.1["args"][1]["raw"], 0);
    assert!(calls.iter().any(|(connection, _)| *connection == 2));
}

#[tokio::test]
async fn fragment_deadline_covers_all_parts_without_resetting_on_progress() {
    let pause = Arc::new(Pause {
        verb: "replay",
        raw: true,
        armed: AtomicBool::new(false),
        arrived: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let state = Arc::new(Mutex::new(Memory::default()));
    let stored = Arc::clone(&state);
    let fake = Fake::paused(
        "fragment-deadline",
        move |call| stored.lock().expect("memory").answer(call),
        Some(Arc::clone(&pause)),
    )
    .await;
    let mut scribe = fake.scribe("A").await;
    scribe.reserve_writer(true).await.expect("reserve");
    scribe
        .settle(Cursor(1), &user("before"), &Beside::default())
        .await
        .expect("seed");
    lose(&mut scribe, &state, 2, false).await;
    state.lock().expect("memory").fragments = Some(Pages::new());
    let witness = scribe.sent.clone();
    let uncertain = scribe.uncertain;
    pause.armed.store(true, Ordering::SeqCst);
    let mut recovering = Box::pin(scribe.recover_writer());
    for nth in 0..2 {
        tokio::select! {
            result = &mut recovering => panic!("premature completion: {result:?}"),
            arrived = tokio::time::timeout(std::time::Duration::from_secs(2), pause.arrived.notified()) => arrived.expect("fragment read"),
        }
        if nth == 0 {
            tokio::time::pause();
            tokio::time::advance(DURABLE - std::time::Duration::from_secs(10)).await;
            tokio::time::resume();
            pause.armed.store(true, Ordering::SeqCst);
            pause.release.notify_one();
        }
    }
    assert_eq!(raw_calls(&fake).len(), 2);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(11)).await;
    let error = recovering.await.expect_err("whole-record deadline");
    tokio::time::resume();
    pause.release.notify_one();
    assert!(error.is_fatal());
    assert!(error.to_string().contains("total read deadline"), "{error}");
    assert_eq!(scribe.sent, witness);
    assert_eq!(scribe.uncertain, uncertain);
    assert!(scribe.check_writer().is_err());
}
