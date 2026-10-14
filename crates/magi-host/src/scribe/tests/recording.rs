//! Recording against a memory layer that misbehaves: a lost reply, a refusal, an entry too large
//! for one frame, and one that is not there at all.

use super::super::*;
use magi_ipc::family::{MAX_FRAME_BYTES, Reply};
use magi_model::scratch::Scratch;
use magi_proto::MessageId;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the fake does with one call: answer it, or hang up without a word.
enum Says {
    Answer(Reply),
    HangUp,
}

/// A memory layer that hands each call, with the number of the connection it came on, to `says`.
struct Fake {
    dir: Scratch,
    calls: Arc<Mutex<Vec<Value>>>,
}

impl Fake {
    fn start(name: &str, says: impl Fn(usize, &Value) -> Says + Send + Sync + 'static) -> Self {
        let dir = Scratch::new("magi-scribe-fake", name);
        let listener = tokio::net::UnixListener::bind(dir.join("m.sock")).expect("bind");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        tokio::spawn(async move {
            let mut nth = 0;
            while let Ok((mut stream, _)) = listener.accept().await {
                while let Ok(length) = stream.read_u32().await {
                    let mut body = vec![0; length as usize];
                    if stream.read_exact(&mut body).await.is_err() {
                        break;
                    }
                    let call: Value = magi_ipc::Wire::read(&body).expect("a family call");
                    seen.lock().expect("calls").push(call.clone());
                    let Says::Answer(reply) = says(nth, &call) else {
                        break;
                    };
                    let bytes = reply.encode(magi_ipc::Wire::Json);
                    let _ = stream.write_u32(bytes.len() as u32).await;
                    let _ = stream.write_all(&bytes).await;
                }
                nth += 1;
            }
        });
        Self { dir, calls }
    }

    fn path(&self) -> std::path::PathBuf {
        self.dir.join("m.sock")
    }

    async fn scribe(&self) -> Scribe {
        let family = Family::dial(self.path()).await.expect("dial");
        Scribe::over(family, Some(self.path()), &SessionId::new("s"))
    }

    /// Each call's verb and cursor, in the order they arrived.
    fn heard(&self) -> Vec<(String, u64)> {
        self.calls
            .lock()
            .expect("calls")
            .iter()
            .map(|call| {
                let verb = call["call"].as_str().unwrap_or_default().to_owned();
                (verb, call["args"][1]["cursor"].as_u64().unwrap_or(0))
            })
            .collect()
    }
}

fn said(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new("u"),
        text: text.into(),
        aside: String::new(),
    }
}

#[tokio::test]
async fn an_observe_whose_reply_was_lost_is_sent_again_and_then_amended() {
    // The first connection takes the observe and hangs up before answering; the store has it.
    let fake = Fake::start("lost", |nth, _| match nth {
        0 => Says::HangUp,
        _ => Says::Answer(Reply::done()),
    });
    let mut scribe = fake.scribe().await;
    scribe
        .settle(Cursor(1), &said("one"), &Beside::default())
        .await
        .expect("the retry lands");
    scribe
        .settle(Cursor(1), &said("one, revised"), &Beside::default())
        .await
        .expect("the revision lands");
    assert_eq!(
        fake.heard(),
        vec![
            ("observe".into(), 1),
            ("observe".into(), 1),
            ("amend".into(), 1)
        ],
        "a cursor once acknowledged is amended, never observed again"
    );
}

#[tokio::test]
async fn a_cursor_the_store_already_holds_is_amended_rather_than_lost() {
    let fake = Fake::start("holds", |_, call| match call["call"].as_str() {
        Some("observe") => Says::Answer(Reply::refused("cursor 4 already holds a different turn")),
        _ => Says::Answer(Reply::done()),
    });
    let mut scribe = fake.scribe().await;
    scribe
        .settle(Cursor(4), &said("changed since"), &Beside::default())
        .await
        .expect("amended");
    assert_eq!(
        fake.heard(),
        vec![("observe".into(), 4), ("amend".into(), 4)]
    );
}

#[tokio::test]
async fn one_entry_refused_does_not_hold_back_the_ones_after_it() {
    let fake = Fake::start("refused", |_, call| {
        match call["args"][1]["cursor"].as_u64() {
            Some(2) => Says::Answer(Reply::refused("not this one")),
            _ => Says::Answer(Reply::done()),
        }
    });
    let session = tokio::sync::Mutex::new(crate::session::Session::recorded(
        SessionId::new("s"),
        Vec::new(),
    ));
    for text in ["one", "two", "three"] {
        session.lock().await.commit(said(text)).expect("commit");
    }
    let mut scribe = Some(fake.scribe().await);
    let flushed = flush(&session, &mut scribe).await;
    assert!(matches!(flushed, Err(Fault::Refused(_))), "{flushed:?}");
    assert_eq!(
        fake.heard(),
        vec![
            ("observe".into(), 1),
            ("observe".into(), 2),
            ("observe".into(), 3)
        ]
    );
    assert!(
        !session.lock().await.has_pending(),
        "the queue is not wedged"
    );
}

#[test]
fn an_entry_past_the_frame_cap_is_recorded_cut_down_and_still_replays() {
    let scribe = Scribe::unconnected("/nonexistent/m.sock".into(), &SessionId::new("s"));
    let huge = said(&"é".repeat(MAX_FRAME_BYTES / 2 + 1));
    let args = scribe
        .fitted("observe", Cursor(1), &huge, &Beside::default())
        .expect("fitted");
    assert!(magi_ipc::family::fits("observe", &args), "it fits now");

    let (cursor, back) = shape::rebuild(&args[1]).expect("the raw still rebuilds");
    assert_eq!(cursor, Cursor(1));
    let Entry::User { text, .. } = back else {
        panic!("a user entry");
    };
    assert!(text.len() < MAX_FRAME_BYTES / 4, "{}", text.len());
    assert!(text.contains("bytes cut here"), "the cut is marked");
}

#[tokio::test]
async fn a_memory_layer_that_is_not_there_yet_is_dialled_again_after_a_pause() {
    let dir = Scratch::new("magi-scribe-late", "one");
    let mut scribe = Scribe::unconnected(dir.join("m.sock"), &SessionId::new("s"));
    assert!(scribe.resumes().await.is_err(), "nothing is there");
    assert!(
        scribe.retry.0 > std::time::Instant::now(),
        "and it backs off"
    );

    let late = Fake::start("late", |_, _| {
        Says::Answer(Reply::of(json!({ "turns": 3 })))
    });
    let mut scribe = Scribe::unconnected(late.path(), &SessionId::new("s"));
    scribe.retry.0 = std::time::Instant::now() + std::time::Duration::from_millis(300);
    let waiting = scribe
        .resumes()
        .await
        .expect_err("within the pause nothing is dialled");
    assert!(waiting.to_string().contains("tried again"), "{waiting}");
    assert!(late.heard().is_empty());

    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    assert_eq!(scribe.resumes().await.expect("answered"), 3);
}

#[test]
fn a_queue_nothing_is_taking_stays_bounded_and_says_what_went() {
    let mut session = crate::session::Session::recorded(SessionId::new("s"), Vec::new());
    let most = crate::session::PENDING_MOST;
    for n in 0..most + 5 {
        session.commit(said(&n.to_string())).expect("commit");
    }
    let told = session.bound_pending().expect("something went");
    assert!(told.contains("5 of the oldest"), "{told}");
    assert!(told.contains("cursors 1 to 5"), "{told}");
    assert_eq!(session.pending_batch().len(), most);
    assert!(
        session.bound_pending().is_none(),
        "said once, when it happens"
    );
}
