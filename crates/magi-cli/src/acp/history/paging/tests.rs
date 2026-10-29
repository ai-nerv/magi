#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use magi_proto::{Entry, MessageId};

fn entry(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new("u"),
        text: text.into(),
        aside: String::new(),
        images: vec![],
    }
}

fn page(after: u64, through: u64, cursors: &[u64], next: Option<u64>) -> HistoryPage {
    HistoryPage {
        session: SessionId::new("owner"),
        transcript: Some("owner@child".into()),
        through: Cursor(through),
        after: Cursor(after),
        next: next.map(Cursor),
        entries: cursors
            .iter()
            .map(|cursor| (Cursor(*cursor), entry(&format!("entry-{cursor}"))))
            .collect(),
    }
}

fn reader(through: u64) -> Paging {
    Paging::new(
        SessionId::new("owner"),
        Cursor(through),
        Cursor::ZERO,
        Some("owner@child".into()),
    )
    .expect("reader")
}

#[tokio::test]
async fn full_native_history_pages_preserve_sparse_order_and_finish_without_an_extra_request() {
    let root = magi_model::scratch::Scratch::new("acp-history", "pages");
    let socket = root.join("host");
    let listener = tokio::net::UnixListener::bind(&socket).expect("fixture socket");
    let cursors: Vec<_> = (1..=300).map(|index| index * 3).collect();
    let expected = page(0, 900, &cursors, None).entries;
    let server = tokio::spawn(async move {
        let mut after = 0;
        for chunk in cursors.chunks(COUNT) {
            let last = *chunk.last().expect("chunk");
            let answer: std::result::Result<HistoryPage, String> =
                Ok(page(after, 900, chunk, (last < 900).then_some(last)));
            let (stream, _) = listener.accept().await.expect("connection");
            let (read, write) = stream.into_split();
            let command = FrameReader::new(read)
                .read::<UiCommand>()
                .await
                .expect("request");
            assert!(
                matches!(command, UiCommand::ReadHistory { session, through:Cursor(900), after:asked, key:Some(key) } if session.as_str() == "owner" && asked == Cursor(after) && key == "controller")
            );
            FrameWriter::new(write).write(&answer).await.expect("page");
            after = last;
        }
    });
    let mut reading = reader(900);
    let mut actual = Vec::new();
    let mut pages = 0;
    while let Some(page) = reading.read(&socket, "controller").await.expect("page") {
        pages += 1;
        actual.extend(page.entries);
    }
    server.await.expect("server");
    assert_eq!(pages, 3);
    assert_eq!(actual, expected);
    assert!(
        reading
            .read(Path::new("/nonexistent/not-opened"), "unused")
            .await
            .expect("complete")
            .is_none()
    );
    assert!(reading.accept(page(900, 900, &[], None)).is_err());
}

#[test]
fn malformed_history_pages_cannot_advance_or_replace_the_expected_transcript() {
    let first = page(0, 100, &[3, 7], Some(7));
    let mut bad = Vec::new();
    for field in [
        "owner",
        "through",
        "after",
        "transcript",
        "missing",
        "unknown-next",
        "ended",
        "empty",
        "repeat",
        "backward",
        "beyond",
        "count",
        "terminal-next",
    ] {
        let mut candidate = first.clone();
        match field {
            "owner" => candidate.session = SessionId::new("other"),
            "through" => candidate.through = Cursor(99),
            "after" => candidate.after = Cursor(1),
            "transcript" => candidate.transcript = Some("owner@other-child".into()),
            "missing" => candidate.transcript = None,
            "unknown-next" => candidate.next = Some(Cursor(8)),
            "ended" => candidate.next = None,
            "empty" => candidate.entries.clear(),
            "repeat" => candidate.entries[1].0 = Cursor(3),
            "backward" => candidate.entries[1].0 = Cursor(2),
            "beyond" => candidate.entries[1].0 = Cursor(101),
            "count" => candidate.entries = vec![(Cursor(1), entry("x")); COUNT + 1],
            _ => {
                candidate.entries[1].0 = Cursor(100);
                candidate.next = Some(Cursor(100));
            }
        }
        bad.push(candidate);
    }
    let mut reading = reader(100);
    for invalid in bad {
        assert!(reading.accept(invalid).is_err());
        assert_eq!(reading.next, Some(Cursor::ZERO));
        assert_eq!(reading.transcript.as_deref(), Some("owner@child"));
    }
    reading
        .accept(first.clone())
        .expect("valid first page after errors");
    assert!(reading.accept(first).is_err(), "repeated first page");
    let mut changed = page(7, 100, &[25, 100], None);
    changed.transcript = Some("other".into());
    assert!(reading.accept(changed).is_err());
    assert_eq!(reading.next, Some(Cursor(7)));
    reading
        .accept(page(7, 100, &[25, 100], None))
        .expect("valid completion after error");
    assert!(reading.next.is_none());
}

#[test]
fn native_history_encoded_byte_limits_count_escaping_before_acceptance() {
    let mut exact = page(0, 3, &[3], None);
    exact.entries[0].1 = entry("");
    let overhead = serde_json::to_vec(&exact).expect("metadata").len();
    exact.entries[0].1 = entry(&"x".repeat(BYTES - overhead));
    assert_eq!(
        serde_json::to_vec(&exact).expect("bounded page").len(),
        BYTES
    );
    reader(3).accept(exact.clone()).expect("exact byte limit");
    let Entry::User { text, .. } = &mut exact.entries[0].1 else {
        panic!("user");
    };
    text.push('x');
    assert!(reader(3).accept(exact).is_err());
    let mut escaped = page(0, 3, &[3], None);
    escaped.entries[0].1 = entry(&"\u{0}".repeat(BYTES / 2));
    assert!(
        reader(3).accept(escaped).is_err(),
        "encoded size, not raw text length"
    );
}

#[test]
fn history_ranges_require_an_identity_and_an_exact_empty_boundary() {
    assert!(Paging::new(SessionId::new("owner"), Cursor(5), Cursor(6), None).is_err());
    assert!(Paging::new(SessionId::new("owner"), Cursor(5), Cursor::ZERO, None).is_err());
    assert!(Paging::new(SessionId::new(""), Cursor::ZERO, Cursor::ZERO, None).is_err());
    assert!(
        Paging::new(
            SessionId::new("x".repeat(513)),
            Cursor::ZERO,
            Cursor::ZERO,
            None
        )
        .is_err()
    );
    let mut empty = reader(0);
    empty
        .accept(page(0, 0, &[], None))
        .expect("empty full history");
    let mut tail = Paging::new(SessionId::new("owner"), Cursor(5), Cursor(5), None)
        .expect("identity-only tail");
    tail.accept(page(5, 5, &[], None))
        .expect("explicit tail completion");
    assert_eq!(tail.transcript.as_deref(), Some("owner@child"));
    assert!(
        reader(5).accept(page(0, 5, &[], None)).is_err(),
        "missing history cannot appear complete"
    );
}
