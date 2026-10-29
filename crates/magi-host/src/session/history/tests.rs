use super::*;
use magi_proto::{Entry, MessageId};

fn entry(n: usize, text: String) -> Entry {
    Entry::User {
        id: MessageId::new(format!("u{n}")),
        text,
        aside: "private context".into(),
        images: vec![],
    }
}

fn page(session: &Session, after: Cursor) -> Result<HistoryPage, String> {
    session.history_page(
        session.id(),
        session.cursor(),
        after,
        Some("durable-transcript".into()),
    )
}

#[test]
fn history_pages_preserve_sparse_cursors_and_signal_exact_completion() {
    let rows: Vec<_> = (1..300)
        .map(|n| (Cursor(n as u64 * 3), entry(n, format!("message {n}"))))
        .collect();
    let session = Session::restored(SessionId::new("owner"), rows.clone()).expect("restore");
    let mut restored = Vec::new();
    let mut after = Cursor::ZERO;
    let mut pages = 0;
    loop {
        let reply = page(&session, after).expect("page");
        assert_eq!(reply.session, *session.id());
        assert_eq!(reply.transcript.as_deref(), Some("durable-transcript"));
        assert_eq!(reply.through, Cursor(897));
        assert_eq!(reply.after, after);
        assert!(reply.entries.len() <= COUNT);
        assert!(serde_json::to_vec(&reply).expect("encode").len() <= BYTES);
        restored.extend(reply.entries);
        pages += 1;
        match reply.next {
            Some(next) => {
                assert!(next > after);
                after = next;
            }
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(restored, rows);
    assert_eq!(session.entries().len(), rows.len());
    assert!(
        page(&session, session.cursor())
            .expect("end")
            .entries
            .is_empty()
    );
}

#[test]
fn history_byte_pages_do_not_silently_skip_or_truncate_entries() {
    let entries: Vec<_> = (0..10).map(|n| entry(n, "\"".repeat(600_000))).collect();
    let session = Session::recorded(SessionId::new("bytes"), entries.clone());
    let first = page(&session, Cursor::ZERO).expect("first");
    assert!(first.entries.len() < entries.len());
    assert!(serde_json::to_vec(&first).expect("wire size").len() <= BYTES);
    let second = page(&session, first.next.expect("continuation")).expect("second");
    assert!(second.next.is_none());
    let restored: Vec<_> = first
        .entries
        .into_iter()
        .chain(second.entries)
        .map(|(_, entry)| entry)
        .collect();
    assert_eq!(restored, entries);
    let oversized = Session::recorded(SessionId::new("large"), vec![entry(1, "x".repeat(BYTES))]);
    assert!(page(&oversized, Cursor::ZERO).is_err());
}

#[test]
fn history_rejects_changed_unknown_busy_and_invalid_boundaries() {
    let mut session = Session::restored(
        SessionId::new("owner"),
        vec![(Cursor(3), entry(1, "first".into()))],
    )
    .expect("restore");
    assert!(
        session
            .history_page(&SessionId::new("other"), Cursor(3), Cursor::ZERO, None)
            .is_err()
    );
    assert!(
        session
            .history_page(session.id(), Cursor(2), Cursor::ZERO, None)
            .is_err()
    );
    assert!(page(&session, Cursor(2)).is_err());
    assert!(page(&session, Cursor(4)).is_err());
    let through = session.cursor();
    session.commit(entry(2, "later".into())).expect("commit");
    assert!(
        session
            .history_page(session.id(), through, Cursor::ZERO, None)
            .is_err()
    );
    session
        .admit(crate::session::admission::Request::Grants(vec![]))
        .expect("active");
    assert!(page(&session, Cursor::ZERO).is_err());
}

#[test]
fn history_empty_state_and_identity_limits_are_explicit() {
    let session = Session::recorded(SessionId::new("empty"), vec![]);
    let empty = session
        .history_page(session.id(), Cursor::ZERO, Cursor::ZERO, None)
        .expect("empty");
    assert!(empty.entries.is_empty() && empty.next.is_none() && empty.transcript.is_none());
    assert!(
        session
            .history_page(
                session.id(),
                Cursor::ZERO,
                Cursor::ZERO,
                Some("x".repeat(513))
            )
            .is_err()
    );
    let long = Session::recorded(SessionId::new("x".repeat(513)), vec![]);
    assert!(page(&long, Cursor::ZERO).is_err());
}

#[test]
fn history_reads_do_not_publish_replayed_entries_or_recount_usage() {
    let session = Session::recorded(
        SessionId::new("read-only"),
        vec![Entry::Assistant {
            id: MessageId::new("a1"),
            text: "answer".into(),
            thinking: "reasoning".into(),
            stop_reason: Some(magi_proto::StopReason::EndTurn),
            error: None,
            signatures: Default::default(),
            usage: magi_proto::Usage {
                cost_micros: 42,
                ..Default::default()
            },
        }],
    );
    let mut events = session.subscribe();
    let before = session.details((0, 0));
    let first = page(&session, Cursor::ZERO).expect("read");
    let second = page(&session, Cursor::ZERO).expect("repeat");
    assert_eq!(first, second);
    assert_eq!(before.cost_micros, session.details((0, 0)).cost_micros);
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}
