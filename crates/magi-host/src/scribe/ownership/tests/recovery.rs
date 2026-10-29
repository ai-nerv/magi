use super::*;
use crate::scribe::{Beside, flush, rebuild};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

mod fragments;

pub(super) struct Pause {
    verb: &'static str,
    raw: bool,
    armed: AtomicBool,
    arrived: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl Pause {
    fn new(verb: &'static str) -> Arc<Self> {
        Arc::new(Self {
            verb,
            raw: false,
            armed: AtomicBool::new(false),
            arrived: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        })
    }

    pub(super) async fn wait(&self, request: &Value) {
        if request["call"] == self.verb
            && (!self.raw || request["args"][1].get("raw").is_some())
            && self.armed.swap(false, Ordering::SeqCst)
        {
            self.arrived.notify_one();
            self.release.notified().await;
        }
    }
}

#[derive(Default)]
struct Memory {
    records: BTreeMap<u64, Value>,
    lose: Option<bool>,
    refuse: Option<&'static str>,
    wrong_run: bool,
    truncated: bool,
    repeated: bool,
    fragments: Option<fragments::Pages>,
}

impl Memory {
    fn answer(&mut self, request: &Value) -> Option<Reply> {
        let verb = request["call"].as_str().expect("verb");
        if self.refuse == Some(verb) {
            return Some(Reply::refused("busy or unsupported"));
        }
        match verb {
            "observe" | "amend" => {
                let landed = self.lose.take();
                if landed != Some(false) {
                    let row = request["args"][1].clone();
                    self.records
                        .insert(row["cursor"].as_u64().expect("cursor"), row);
                }
                landed.is_none().then(Reply::done)
            }
            "replay" => {
                if let Some(pages) = &self.fragments {
                    return Some(pages.reply(&self.records, request));
                }
                let from = if self.repeated {
                    0
                } else {
                    request["args"][1]["from"].as_u64().expect("from")
                };
                let rows = self
                    .records
                    .range(from..)
                    .take(1)
                    .map(|(_, row)| {
                        let mut row = row.clone();
                        if self.truncated {
                            row["truncated"] = json!(true);
                        }
                        row
                    })
                    .collect();
                Some(Reply::rows(rows))
            }
            "resume" if self.wrong_run => Some(Reply::of(json!({"run":"another"}))),
            "resume" => Some(Reply::of(
                json!({"run":self.records.values().next().map(|row| &row["run"]).unwrap_or(&request["args"][0])}),
            )),
            _ => compatible(request),
        }
    }
}

async fn memory() -> (Fake, Arc<Mutex<Memory>>, Scribe) {
    let state = Arc::new(Mutex::new(Memory::default()));
    let held = Arc::clone(&state);
    let fake = Fake::new("recovery", move |request| {
        held.lock().expect("memory").answer(request)
    })
    .await;
    let mut scribe = fake.scribe("A").await;
    scribe.reserve_writer(true).await.expect("reserve");
    (fake, state, scribe)
}

fn user(text: &str) -> Entry {
    let mut out = entry();
    if let Entry::User { text: body, .. } = &mut out {
        *body = text.into();
    }
    out
}

async fn lose(scribe: &mut Scribe, state: &Mutex<Memory>, cursor: u64, landed: bool) {
    state.lock().expect("memory").lose = Some(landed);
    assert!(
        scribe
            .settle(Cursor(cursor), &user("uncertain"), &Beside::default())
            .await
            .expect_err("lost reply")
            .is_fatal()
    );
    assert!(!scribe.family.as_ref().expect("family").has_connection());
}

#[tokio::test]
async fn fresh_reservation_precedes_history_and_pending_handover() {
    for landed in [false, true] {
        let (fake, state, mut scribe) = memory().await;
        let session = tokio::sync::Mutex::new(crate::session::Session::recorded(
            SessionId::new("A"),
            Vec::new(),
        ));
        session.lock().await.commit(user("before")).expect("first");
        let mut held = Some(scribe);
        flush(&session, &mut held).await.expect("first flush");
        state.lock().expect("memory").lose = Some(landed);
        session.lock().await.commit(user("during")).expect("second");
        assert!(
            flush(&session, &mut held)
                .await
                .expect_err("outage")
                .is_fatal()
        );
        let pending = session.lock().await.pending_batch();
        assert_eq!(pending.len(), 1);
        assert!(fake.calls().iter().all(|(connection, _)| *connection == 1));
        flush(&session, &mut held).await.expect("recovered");
        assert!(!session.lock().await.has_pending());
        scribe = held.expect("scribe");
        assert!(scribe.uncertain.is_none());
        assert_eq!(scribe.sent.len(), 2);
        let calls = fake.calls();
        let second: Vec<_> = calls
            .iter()
            .filter(|(connection, _)| *connection == 2)
            .map(|(_, call)| call["call"].as_str().expect("verb"))
            .collect();
        assert_eq!(
            &second[..4],
            &["verbs", "writer_status", "writer", "resume"]
        );
        assert!(
            second[4..second.len() - 1]
                .iter()
                .all(|verb| *verb == "replay")
        );
        assert_eq!(
            second.last(),
            Some(&if landed { "amend" } else { "observe" })
        );
        let records = &state.lock().expect("memory").records;
        assert_eq!(records.len(), 2);
        assert_eq!(rebuild(&records[&1]).expect("first").1, user("before"));
        assert_eq!(rebuild(&records[&2]).expect("second").1, user("during"));
    }
}

#[tokio::test]
async fn uncertain_amend_accepts_only_the_old_or_attempted_record() {
    for landed in [false, true] {
        let (_fake, state, mut scribe) = memory().await;
        scribe
            .settle(Cursor(7), &user("before"), &Beside::default())
            .await
            .expect("seed");
        lose(&mut scribe, &state, 7, landed).await;
        scribe.recover_writer().await.expect("reconciled");
        scribe
            .settle(Cursor(7), &user("latest"), &Beside::default())
            .await
            .expect("updated");
        assert_eq!(state.lock().expect("memory").records.len(), 1);
        assert_eq!(
            rebuild(&state.lock().expect("memory").records[&7])
                .expect("raw")
                .1,
            user("latest")
        );
    }
}

#[tokio::test]
async fn incompatible_history_never_writes_or_discards_pending_entries() {
    for damage in [
        "missing",
        "changed",
        "extra",
        "uncertain",
        "run",
        "truncated",
        "repeated",
    ] {
        let (fake, state, mut scribe) = memory().await;
        scribe
            .settle(Cursor(1), &user("before"), &Beside::default())
            .await
            .expect("seed");
        lose(&mut scribe, &state, 4, true).await;
        {
            let mut stored = state.lock().expect("memory");
            let different =
                crate::scribe::shape::turn(Cursor(1), &user("different"), &Beside::default())
                    .expect("turn");
            match damage {
                "missing" => {
                    stored.records.remove(&1);
                }
                "changed" => {
                    stored.records.insert(1, different);
                }
                "extra" => {
                    stored.records.insert(
                        2,
                        crate::scribe::shape::turn(
                            Cursor(2),
                            &user("other writer"),
                            &Beside::default(),
                        )
                        .expect("extra"),
                    );
                }
                "uncertain" => {
                    stored.records.insert(
                        4,
                        crate::scribe::shape::turn(
                            Cursor(4),
                            &user("unrecognized"),
                            &Beside::default(),
                        )
                        .expect("extra"),
                    );
                }
                "run" => stored.wrong_run = true,
                "truncated" => stored.truncated = true,
                "repeated" => stored.repeated = true,
                _ => unreachable!(),
            }
        }
        let before = scribe.sent.clone();
        let uncertain = scribe.uncertain;
        let session = tokio::sync::Mutex::new(crate::session::Session::recorded(
            SessionId::new("A"),
            Vec::new(),
        ));
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
        assert_eq!(scribe.sent, before);
        assert_eq!(scribe.uncertain, uncertain);
        assert!(
            fake.calls()
                .iter()
                .filter(|(connection, _)| *connection != 1)
                .all(|(_, call)| call["call"] != "observe" && call["call"] != "amend")
        );
    }
}

#[tokio::test]
async fn busy_or_unsupported_recovery_never_downgrades_or_reads_history() {
    for refused in ["verbs", "writer_status", "writer"] {
        let (fake, state, mut scribe) = memory().await;
        lose(&mut scribe, &state, 1, false).await;
        state.lock().expect("memory").refuse = Some(refused);
        assert!(
            scribe
                .recover_writer()
                .await
                .expect_err("refused")
                .is_fatal()
        );
        assert!(scribe.uncertain.is_some());
        let calls = fake.calls();
        assert!(
            calls
                .iter()
                .filter(|(connection, _)| *connection != 1)
                .all(|(_, call)| !["replay", "resume", "observe", "amend"]
                    .contains(&call["call"].as_str().expect("verb")))
        );
        assert!(
            scribe
                .recover_writer()
                .await
                .expect_err("backoff")
                .is_fatal()
        );
        assert_eq!(fake.calls(), calls);
    }
}

#[tokio::test]
async fn changed_attempt_after_loss_cannot_replace_the_uncertain_fingerprint() {
    let (fake, state, mut scribe) = memory().await;
    lose(&mut scribe, &state, 1, true).await;
    let uncertain = scribe.uncertain;
    let before = fake.calls();
    assert!(
        scribe
            .settle(Cursor(1), &user("different"), &Beside::default())
            .await
            .is_err()
    );
    assert_eq!(scribe.uncertain, uncertain);
    assert_eq!(fake.calls(), before);
    scribe.recover_writer().await.expect("recover original");
}

#[tokio::test]
async fn cancellation_during_reacquisition_or_validation_preserves_old_state() {
    for verb in ["writer", "resume", "replay"] {
        let pause = Pause::new(verb);
        let state = Arc::new(Mutex::new(Memory::default()));
        let held = Arc::clone(&state);
        let fake = Fake::paused(
            "cancel-recovery",
            move |request| held.lock().expect("memory").answer(request),
            Some(Arc::clone(&pause)),
        )
        .await;
        let mut scribe = fake.scribe("A").await;
        scribe.reserve_writer(true).await.expect("reserve");
        lose(&mut scribe, &state, 1, true).await;
        let uncertain = scribe.uncertain;
        pause.armed.store(true, Ordering::SeqCst);
        let mut recovering = Box::pin(scribe.recover_writer());
        tokio::select! {
            result = &mut recovering => panic!("recovery completed before cancellation: {result:?}"),
            arrived = tokio::time::timeout(std::time::Duration::from_secs(2), pause.arrived.notified()) => arrived.expect("paused exchange"),
        }
        drop(recovering);
        pause.release.notify_one();
        assert_eq!(scribe.uncertain, uncertain);
        assert!(scribe.sent.is_empty());
        assert!(scribe.check_writer().is_err());
        let calls = fake.calls();
        assert!(
            scribe
                .recover_writer()
                .await
                .expect_err("backoff after cancellation")
                .is_fatal()
        );
        assert_eq!(calls, fake.calls());
        scribe.retry.0 = std::time::Instant::now();
        scribe.recover_writer().await.expect("fresh recovery");
        assert!(scribe.check_writer().is_ok());
        assert_eq!(scribe.sent.len(), 1);
        assert!(
            fake.calls()
                .iter()
                .filter(|(connection, _)| *connection != 1)
                .all(|(_, call)| call["call"] != "observe" && call["call"] != "amend")
        );
    }
}

#[tokio::test]
async fn an_empty_child_transcript_can_recover_before_its_run_binding_lands() {
    let state = Arc::new(Mutex::new(Memory::default()));
    let held = Arc::clone(&state);
    let fake = Fake::new("child-recovery", move |request| {
        held.lock().expect("memory").answer(request)
    })
    .await;
    let mut scribe = fake.scribe("A").await.recording_as("child-key".into());
    scribe.reserve_writer(true).await.expect("reserve");
    lose(&mut scribe, &state, 1, false).await;
    scribe.recover_writer().await.expect("empty child");
    scribe
        .settle(Cursor(1), &user("child"), &Beside::default())
        .await
        .expect("bind run");
    assert_eq!(state.lock().expect("memory").records[&1]["run"], "A");
}
