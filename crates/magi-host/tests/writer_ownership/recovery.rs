use super::*;
use magi_host::scribe::flush;
use tokio::sync::Mutex;

async fn recorded(memory: &Serving) -> (Mutex<Session>, Option<Scribe>) {
    let mut owned = scribe(memory, "A").await;
    owned.reserve_writer(true).await.expect("reserve");
    let session = Mutex::new(Session::recorded(SessionId::new("A"), Vec::new()));
    session.lock().await.commit(entry("before")).expect("first");
    let mut held = Some(owned);
    flush(&session, &mut held).await.expect("record first");
    (session, held)
}

async fn outage(memory: Serving, session: &Mutex<Session>, held: &mut Option<Scribe>) {
    drop(memory);
    session.lock().await.commit(entry("during")).expect("queue");
    assert!(
        flush(session, held)
            .await
            .expect_err("dead memory")
            .is_fatal()
    );
    assert!(session.lock().await.has_pending());
}

#[tokio::test]
async fn selected_memory_restart_recovers_pending_rows_under_a_fresh_reservation() {
    let dir = Scratch::new("mwo", "restart");
    let Some(memory) = Serving::start(&dir, "restart").await else {
        return;
    };
    let (session, mut held) = recorded(&memory).await;
    outage(memory, &session, &mut held).await;
    let memory = Serving::start(&dir, "restart")
        .await
        .expect("restart memory");
    flush(&session, &mut held).await.expect("recover recording");
    assert!(!session.lock().await.has_pending());
    unavailable(&memory, "A").await;
    session
        .lock()
        .await
        .commit(entry("after"))
        .expect("continue");
    flush(&session, &mut held)
        .await
        .expect("continued recording");
    assert_eq!(
        scribe(&memory, "A")
            .await
            .replay()
            .await
            .expect("durable history"),
        vec![
            (Cursor(1), entry("before")),
            (Cursor(2), entry("during")),
            (Cursor(3), entry("after")),
        ]
    );
    drop(held);
    let released = acquire(&memory, "A").await;
    drop(released);
}

#[tokio::test]
async fn selected_memory_competing_writer_and_changed_history_keep_the_queue() {
    let dir = Scratch::new("mwo", "conflict");
    let Some(memory) = Serving::start(&dir, "conflict").await else {
        return;
    };
    let (session, mut held) = recorded(&memory).await;
    outage(memory, &session, &mut held).await;
    let memory = Serving::start(&dir, "conflict")
        .await
        .expect("restart memory");
    let mut rival = acquire(&memory, "A").await;
    assert!(
        flush(&session, &mut held)
            .await
            .expect_err("busy owner")
            .is_fatal()
    );
    assert!(session.lock().await.has_pending());
    rival
        .observe(Cursor(8), &entry("rival"), &Default::default())
        .await
        .expect("other owner writes");
    drop(rival);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        flush(&session, &mut held)
            .await
            .expect_err("history changed")
            .is_fatal()
    );
    assert!(session.lock().await.has_pending());
    assert_eq!(
        scribe(&memory, "A")
            .await
            .replay()
            .await
            .expect("unchanged history"),
        vec![(Cursor(1), entry("before")), (Cursor(8), entry("rival")),]
    );
    let released = acquire(&memory, "A").await;
    drop(released);
}

#[tokio::test]
async fn selected_memory_large_records_survive_loading_and_repeated_outage_recovery() {
    let dir = Scratch::new("mwo", "large-record");
    let Some(mut memory) = Serving::start(&dir, "large-record").await else {
        return;
    };
    let mut owned = scribe(&memory, "A").await;
    owned.reserve_writer(true).await.expect("reserve");
    let large = Entry::User {
        id: MessageId::new("large"),
        text: "\u{0001}".repeat(1_300_000),
        aside: String::new(),
        images: Vec::new(),
    };
    let mut session = Mutex::new(Session::recorded(SessionId::new("A"), Vec::new()));
    session
        .lock()
        .await
        .commit(large.clone())
        .expect("large entry");
    let mut held = Some(owned);
    flush(&session, &mut held).await.expect("large write");
    let mut probe = Family::dial(memory.socket()).await.expect("inspection");
    let values = probe
        .call(
            "replay",
            vec![
                serde_json::json!("A"),
                serde_json::json!({"from":0,"bytes":4*1024*1024}),
            ],
        )
        .await
        .expect("paged summary");
    let first = values.first().expect("result");
    let row = first
        .as_array()
        .and_then(|rows| rows.first())
        .unwrap_or(first);
    assert_eq!(row.get("truncated"), Some(&serde_json::Value::Bool(true)));
    assert!(
        row.get("raw_bytes")
            .and_then(serde_json::Value::as_u64)
            .expect("raw length")
            > 4 * 1024 * 1024
    );
    drop(probe);
    let mut expected = vec![(Cursor(1), large)];
    for label in ["fresh", "loaded"] {
        drop(memory);
        let cursor = session.lock().await.commit(entry(label)).expect("queue");
        expected.push((cursor, entry(label)));
        assert!(
            flush(&session, &mut held)
                .await
                .expect_err("outage")
                .is_fatal()
        );
        assert!(session.lock().await.has_pending());
        memory = Serving::start(&dir, "large-record").await.expect("restart");
        flush(&session, &mut held).await.expect("complete recovery");
        assert!(!session.lock().await.has_pending());
        assert_eq!(
            held.as_mut()
                .expect("scribe")
                .replay()
                .await
                .expect("complete replay"),
            expected
        );
        unavailable(&memory, "A").await;
        if label == "fresh" {
            drop(held.take());
            drop(memory);
            memory = Serving::start(&dir, "large-record")
                .await
                .expect("loading daemon");
            let (loaded, rows) = scribe(&memory, "unused")
                .await
                .load_for("A", true)
                .await
                .expect("complete load");
            assert_eq!(rows, expected);
            session = Mutex::new(Session::restored(SessionId::new("A"), rows).expect("restore"));
            held = Some(loaded);
        }
    }
    drop(held);
    drop(acquire(&memory, "A").await);
}
