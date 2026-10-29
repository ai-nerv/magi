use super::*;

pub(super) fn prompt(text: &str) -> UiCommand {
    UiCommand::SubmitPrompt {
        text: text.into(),
        aside: String::new(),
        images: Vec::new(),
    }
}

pub(super) fn fixture() -> (Preparing, Arc<Mutex<Session>>) {
    let session = Arc::new(Mutex::new(Session::recorded(
        magi_proto::SessionId::new("preparation"),
        Vec::new(),
    )));
    let preparing = Preparing::new(
        Arc::clone(&session),
        Arc::new(RwLock::new(None)),
        Arc::new(Models::new(Catalog::empty())),
        Arc::new(Mutex::new(None)),
    );
    (preparing, session)
}

pub(super) fn queued(preparing: &Preparing, text: &str) -> Ticket {
    let Queued::Waiting(ticket) = preparing.queue(prompt(text)).expect("queued") else {
        panic!("waiting");
    };
    ticket
}

pub(super) async fn released(preparing: &Preparing) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while preparing.count.available_permits() != COUNT
            || preparing.bytes.available_permits() != BYTES
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("budget release");
}

#[tokio::test]
async fn tracked_preparation_preserves_identity_through_immediate_and_cancelled_paths() {
    let (preparing, session) = fixture();
    let tracked = |request, text: &str| UiCommand::SubmitTracked {
        request,
        text: text.into(),
        images: vec![],
        aside: String::new(),
    };
    assert!(matches!(
        preparing.queue(tracked(7, "ordinary")),
        Ok(Queued::Immediate(_, Some(7), _))
    ));
    let held = preparing.epoch.boundary.lock().await;
    let Queued::Waiting(mut ticket) = preparing.queue(tracked(8, "/skill review")).expect("queue")
    else {
        panic!("waiting");
    };
    assert_eq!(ticket.tracking, Some(8));
    preparing.invalidate();
    drop(held);
    assert!(ticket.receive().await.is_err());
    assert!(session.lock().await.entries().is_empty());
    drop(ticket);
    released(&preparing).await;
}

#[tokio::test]
async fn ordinary_prompts_stay_immediate_and_malformed_invocations_fail_without_work() {
    let (preparing, session) = fixture();
    assert!(matches!(
        preparing.queue(prompt("ordinary")),
        Ok(Queued::Immediate(..))
    ));
    assert!(matches!(
        preparing.queue(prompt("/skills literal")),
        Ok(Queued::Immediate(..))
    ));
    assert!(preparing.queue(prompt("/skill ../escape")).is_err());
    assert!(
        preparing
            .queue(prompt(&format!(
                "/skill review {}",
                "x".repeat(4 * 1024 * 1024)
            )))
            .is_err()
    );
    assert!(session.lock().await.entries().is_empty());
    released(&preparing).await;
}

#[tokio::test]
async fn dropping_a_ticket_cancels_unaccepted_work_and_releases_both_budgets() {
    let (preparing, session) = fixture();
    let held = preparing.epoch.boundary.lock().await;
    let ticket = queued(&preparing, "/skill review");
    assert_eq!(preparing.count.available_permits(), COUNT - 1);
    drop(ticket);
    drop(held);
    released(&preparing).await;
    assert!(session.lock().await.entries().is_empty());
}

#[tokio::test]
async fn invalidation_refuses_the_old_generation_and_preserves_new_commands() {
    let (preparing, session) = fixture();
    let held = preparing.epoch.boundary.lock().await;
    let mut first = queued(&preparing, "/skill review");
    let mut second = queued(&preparing, "queued behind it");
    preparing.invalidate();
    drop(held);
    assert!(first.receive().await.is_err());
    assert!(second.receive().await.is_err());
    assert_eq!(preparing.count.available_permits(), COUNT - 2);
    assert!(session.lock().await.entries().is_empty());
    drop((first, second));
    released(&preparing).await;
    assert!(matches!(
        preparing.queue(prompt("next")),
        Ok(Queued::Immediate(..))
    ));
}

#[tokio::test]
async fn count_and_encoded_byte_budgets_apply_to_waiting_and_completed_tickets() {
    let (preparing, _) = fixture();
    let held = preparing.epoch.boundary.lock().await;
    let mut tickets = vec![queued(&preparing, "/skill review")];
    for _ in 1..COUNT {
        tickets.push(queued(&preparing, "ordinary"));
    }
    assert!(preparing.queue(prompt("past count")).is_err());
    preparing.invalidate();
    drop(held);
    for ticket in &mut tickets {
        assert!(ticket.receive().await.is_err());
    }
    assert_eq!(preparing.count.available_permits(), 0);
    drop(tickets);
    released(&preparing).await;
    let held = preparing.epoch.boundary.lock().await;
    let mut tickets = vec![queued(&preparing, "/skill review")];
    let large = "x".repeat(3 * 1024 * 1024);
    while let Ok(Queued::Waiting(ticket)) = preparing.queue(prompt(&large)) {
        tickets.push(ticket);
    }
    assert!(tickets.len() < COUNT);
    assert!(preparing.bytes.available_permits() < large.len());
    preparing.invalidate();
    drop(tickets);
    drop(held);
    released(&preparing).await;
}

#[tokio::test(start_paused = true)]
async fn queued_deadline_starts_at_enqueue_not_when_acquisition_begins() {
    let (preparing, session) = fixture();
    let held = preparing.epoch.boundary.lock().await;
    let mut ticket = queued(&preparing, "/skill review");
    tokio::time::advance(std::time::Duration::from_secs(46)).await;
    assert!(
        ticket
            .receive()
            .await
            .expect_err("deadline")
            .contains("deadline")
    );
    drop(held);
    assert!(session.lock().await.entries().is_empty());
}

#[tokio::test]
async fn absent_history_refuses_workflow_but_following_ordinary_prompts_keep_fifo() {
    let (preparing, session) = fixture();
    let held = preparing.epoch.boundary.lock().await;
    let mut workflow = queued(&preparing, "/skill review");
    let mut first = queued(&preparing, "first");
    let mut second = queued(&preparing, "second");
    drop(held);
    assert!(
        workflow
            .receive()
            .await
            .expect_err("history missing")
            .contains("history store")
    );
    assert!(first.receive().await.is_ok());
    assert!(second.receive().await.is_ok());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let texts: Vec<_> = session
                .lock()
                .await
                .entries()
                .iter()
                .filter_map(|entry| match entry {
                    Entry::User { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            if texts.len() == 2 {
                assert_eq!(texts, ["first", "second"]);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("ordered prompts");
}

#[tokio::test]
async fn catalog_reply_reservations_remain_charged_until_completed_tickets_are_consumed() {
    let (preparing, _) = fixture();
    let held = preparing.epoch.boundary.lock().await;
    let mut tickets = Vec::new();
    for _ in 0..BYTES / (listing::MOST + 512) {
        let Queued::Waiting(ticket) = preparing
            .queue(UiCommand::RefreshWorkflows)
            .expect("listing queued")
        else {
            panic!("waiting");
        };
        assert!(ticket.listing);
        tickets.push(ticket);
    }
    assert!(preparing.queue(UiCommand::RefreshWorkflows).is_err());
    preparing.invalidate();
    drop(held);
    for ticket in &mut tickets {
        assert!(ticket.receive().await.is_err());
    }
    assert!(preparing.queue(UiCommand::RefreshWorkflows).is_err());
    drop(tickets);
    released(&preparing).await;
}
