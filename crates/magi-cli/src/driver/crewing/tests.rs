//! The funnel every write goes through, and what the footer says about whose screen this is.

use super::*;
use crate::melchior::Peer;

/// A peer with a screen, as melchior publishes one.
fn peer(id: &str) -> Peer {
    Peer {
        id: id.to_owned(),
        role: "reviewer".to_owned(),
        ui: Some(std::path::PathBuf::from(format!("/run/magi/{id}.host"))),
    }
}

fn app_with(reachable: Vec<Peer>) -> App {
    let mut app = App::new();
    app.named = "axum/main/alpha-rho".to_owned();
    app.reachable = reachable;
    app
}

/// Everything a person can type reaches the session they are looking at, and nothing else does.
#[tokio::test]
async fn a_prompt_typed_at_a_peer_is_refused_and_said_so() {
    let mut app = app_with(vec![peer("beta-nu")]);
    let (to, mut sent) = mpsc::channel(8);
    app.attach_to(Some(peer("beta-nu")));

    direct(
        &mut app,
        &to,
        UiCommand::SubmitPrompt {
            text: "do the thing".into(),
            aside: String::new(),
        },
    )
    .await;

    assert!(
        sent.try_recv().is_err(),
        "a prompt reached a session this screen is only reading"
    );
    assert_eq!(app.entries().len(), 1, "and nothing said why");
}

/// The same funnel, on our own session, sends everything.
#[tokio::test]
async fn our_own_session_takes_what_it_is_given() {
    let mut app = app_with(vec![peer("beta-nu")]);
    let (to, mut sent) = mpsc::channel(8);
    direct(&mut app, &to, UiCommand::Interrupt).await;
    assert_eq!(sent.try_recv(), Ok(UiCommand::Interrupt));
    assert!(app.entries().is_empty(), "and says nothing about it");
}

/// A width is machinery, not a keystroke: refused, and without a line above the prompt.
#[tokio::test]
async fn the_width_of_this_window_is_never_offered_to_a_peer() {
    let mut app = app_with(vec![peer("beta-nu")]);
    let (to, mut sent) = mpsc::channel(8);
    app.attach_to(Some(peer("beta-nu")));

    direct(
        &mut app,
        &to,
        UiCommand::Sized {
            rows: Some(4),
            cols: 80,
            holds: false,
        },
    )
    .await;

    assert!(sent.try_recv().is_err());
    assert!(app.entries().is_empty(), "{:?}", app.entries());
}

/// What melchior hands this session is this session's, wherever the screen happens to be.
#[tokio::test]
async fn a_message_that_arrived_while_we_were_away_is_delivered_when_we_are_back() {
    let mut app = app_with(vec![peer("beta-nu")]);
    let (to, mut sent) = mpsc::channel(8);
    let (target, _watching) = watch::channel(std::path::PathBuf::from("/run/magi/own.host"));
    let mut held = Vec::new();
    app.attach_to(Some(peer("beta-nu")));

    let arrived = app.received("axum/main/tau-mu", "note", "the parser is done");
    ours(&app, &to, &mut held, arrived).await;
    assert!(sent.try_recv().is_err(), "not onto somebody else's socket");
    assert_eq!(held.len(), 1);

    walk(
        &mut app,
        true,
        std::path::Path::new("/run/magi/own.host"),
        &target,
        &to,
        &mut held,
    )
    .await;

    assert!(app.attached.is_none(), "the ring came home");
    assert!(
        matches!(sent.try_recv(), Ok(UiCommand::Arrived { .. })),
        "the message was let go"
    );
    assert!(held.is_empty());
}

/// The footer names whoever is on screen, and counts what the arrows can reach.
#[test]
fn the_footer_follows_the_screen_rather_than_the_process() {
    let mut app = app_with(vec![peer("beta-nu"), peer("tau-mu")]);
    let mine = footer_data(&app);
    assert_eq!(mine.identity, "axum/main/alpha-rho");
    assert_eq!(mine.crew, 3);
    assert!(mine.own);

    app.attach_to(Some(peer("beta-nu")));
    let theirs = footer_data(&app);
    assert_eq!(theirs.identity, "reviewer/beta-nu");
    assert!(!theirs.own, "the one signal that this is not your session");
}

/// And a session alone in its project draws no control, because there is nowhere to go.
#[test]
fn a_session_that_started_nothing_has_a_crew_of_one() {
    let app = app_with(vec![]);
    assert_eq!(footer_data(&app).crew, 1);
}
