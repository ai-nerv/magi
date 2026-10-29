#![allow(clippy::expect_used)]

use magi_host::{catalog::Catalog, scribe::Scribe, session::Session};
use magi_ipc::{FrameReader, FrameWriter, family::Family};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, Entry, HarnessEvent, MessageId, SessionId, UiCommand};
use magi_testkit::memory::Serving;
use tokio::net::unix::OwnedReadHalf;

fn entry(id: &str) -> Entry {
    Entry::User {
        id: MessageId::new(id),
        text: id.into(),
        aside: String::new(),
        images: Vec::new(),
    }
}

async fn scribe(memory: &Serving, id: &str) -> Scribe {
    Scribe::over(
        Family::dial(memory.socket())
            .await
            .expect("memory connection"),
        None,
        &SessionId::new(id),
    )
}

async fn unavailable(memory: &Serving, id: &str) {
    assert!(scribe(memory, id).await.reserve_writer(true).await.is_err());
    assert!(
        scribe(memory, id)
            .await
            .observe(Cursor(99), &entry("forbidden"), &Default::default())
            .await
            .is_err()
    );
}

async fn acquire(memory: &Serving, id: &str) -> Scribe {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let mut next = scribe(memory, id).await;
            if matches!(next.reserve_writer(true).await, Ok(true)) {
                return next;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("previous owner released the transcript")
}

async fn event(
    reader: &mut FrameReader<OwnedReadHalf>,
    accepts: impl Fn(&HarnessEvent) -> bool,
) -> HarnessEvent {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let next = reader.read::<HarnessEvent>().await.expect("host event");
            if accepts(&next) {
                return next;
            }
        }
    })
    .await
    .expect("host event deadline")
}

struct Host(tokio::task::JoinHandle<Result<(), magi_host::HostError>>);

impl Drop for Host {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn prepared_writer_survives_handoff_same_key_and_failed_resume_until_shutdown() {
    lifecycle(true).await;
}

#[tokio::test]
async fn ordinary_host_reserves_and_transfers_its_writer_until_shutdown() {
    lifecycle(false).await;
}

async fn lifecycle(prepared: bool) {
    let dir = Scratch::new("mwo", if prepared { "prepared" } else { "ordinary" });
    let Some(memory) = Serving::start(&dir, "ownership").await else {
        return;
    };
    for id in ["A", "B"] {
        scribe(&memory, id)
            .await
            .observe(Cursor(1), &entry(id), &Default::default())
            .await
            .expect("seed transcript");
    }
    let path = dir.join("host.sock");
    let listener = magi_ipc::bind(&path).await.expect("bind host");
    let session = Session::recorded(SessionId::new("A"), vec![entry("A")]);
    let mut host = if prepared {
        let (loaded, rows) = scribe(&memory, "unused")
            .await
            .load_for("A", true)
            .await
            .expect("reserve before replay");
        assert_eq!(rows, vec![(Cursor(1), entry("A"))]);
        unavailable(&memory, "A").await;
        Host(tokio::spawn(magi_host::serve_recording(
            listener,
            session,
            None,
            Catalog::empty(),
            loaded,
        )))
    } else {
        Host(tokio::spawn(magi_host::serve_on(
            listener,
            session,
            None,
            Catalog::empty(),
            Some(memory.socket().to_owned()),
        )))
    };
    let (read, write) = magi_ipc::connect(&path).await.expect("attach").into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor(0),
            draws: false,
            key: magi_ipc::read_key(&path),
        })
        .await
        .expect("authenticate controller");
    event(&mut reader, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    unavailable(&memory, "A").await;
    writer
        .write(&UiCommand::Resume { id: "A".into() })
        .await
        .expect("same key resume");
    event(&mut reader, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { session, entries, .. }
        if session.as_str() == "A" && entries == &vec![entry("A")])
    })
    .await;
    unavailable(&memory, "A").await;

    let other = acquire(&memory, "B").await;
    writer
        .write(&UiCommand::Resume { id: "B".into() })
        .await
        .expect("busy resume");
    event(&mut reader, |e| {
        matches!(e, HarnessEvent::Refused { message, .. }
        if message.contains("cannot resume"))
    })
    .await;
    unavailable(&memory, "A").await;
    unavailable(&memory, "B").await;
    drop(other);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if scribe(&memory, "B")
                .await
                .observe(Cursor(1), &entry("B"), &Default::default())
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("B released before resume");
    writer
        .write(&UiCommand::Resume { id: "B".into() })
        .await
        .expect("resume B");
    event(&mut reader, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { session, entries, .. }
        if session.as_str() == "B" && entries == &vec![entry("B")])
    })
    .await;
    let released_a = acquire(&memory, "A").await;
    unavailable(&memory, "B").await;
    drop((reader, writer));
    host.0.abort();
    assert!(
        (&mut host.0)
            .await
            .as_ref()
            .is_err_and(|why| why.is_cancelled())
    );
    let mut released_b = acquire(&memory, "B").await;
    assert_eq!(
        released_b.replay().await.expect("B remains intact"),
        vec![(Cursor(1), entry("B"))]
    );
    drop(released_a);
}
