//! Moving between agents, and the two things that must be true before the arrows exist.

use super::super::*;
use crate::melchior::Peer;
use magi_proto::UiCommand;

/// A peer with a screen, as melchior publishes one.
fn peer(id: &str, role: &str) -> Peer {
    Peer {
        id: id.to_owned(),
        role: role.to_owned(),
        ui: Some(std::path::PathBuf::from(format!("/run/magi/{id}.host"))),
    }
}

/// This session, plus whoever melchior says is listening.
fn app_with(reachable: Vec<Peer>) -> App {
    let mut app = App::new();
    app.named = "axum/main/alpha-rho".to_owned();
    app.reachable = reachable;
    app
}

/// **The sharpest bug in the feature, and it never presents as an error.**
///
/// `apply` folds the cursor forward with `max` and never back. A screen that had reached event
/// 400 of its own session asks a peer for everything after *its* four-hundredth, and `snapshot`
/// answers by taking the first four hundred entries the peer does not have — so the peer arrives
/// with no history at all. Nothing anywhere says so; it reads as an agent that has said nothing.
#[test]
fn attaching_somewhere_else_forgets_where_the_last_one_had_got_to() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer")]);
    app.apply(HarnessEvent::StatusChanged {
        cursor: Cursor(400),
        status: AgentStatus::Idle,
    });
    assert_eq!(app.cursor(), Cursor(400), "the fold happened");

    app.attach_to(Some(peer("beta-nu", "reviewer")));

    assert_eq!(
        app.cursor(),
        Cursor::ZERO,
        "a stale cursor is what truncates the peer's history to nothing"
    );
}

/// And everything else on screen belonged to the agent that sent it.
#[test]
fn a_swap_takes_the_whole_screen_with_it() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer")]);
    app.apply(HarnessEvent::UserMessage {
        cursor: Cursor(1),
        id: MessageId::new("u1"),
        text: "mine".into(),
    });
    app.show_notice("also mine".to_owned());
    app.waiting = 3;

    app.attach_to(Some(peer("beta-nu", "reviewer")));

    assert!(app.entries().is_empty(), "{:?}", app.entries());
    assert_eq!(app.unanswered(), 0, "the count was against our transcript");
    assert!(app.overlay.is_none());
}

/// Coming home is a swap like any other, and forgets the peer just as completely.
#[test]
fn coming_back_forgets_the_peer_as_well() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer")]);
    app.attach_to(Some(peer("beta-nu", "reviewer")));
    app.apply(HarnessEvent::StatusChanged {
        cursor: Cursor(90),
        status: AgentStatus::Idle,
    });

    app.attach_to(None);

    assert_eq!(app.cursor(), Cursor::ZERO);
    assert!(app.entries().is_empty());
    assert!(app.attached.is_none());
}

/// The ring: this session first, then the peers, then round to this session again.
#[test]
fn the_arrows_walk_a_ring_that_starts_and_ends_at_home() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer"), peer("tau-mu", "scribe")]);
    assert_eq!(app.crew_size(), 3);

    assert_eq!(
        app.step(true),
        Some(Seat::Peer("/run/magi/beta-nu.host".into()))
    );
    assert_eq!(
        app.step(true),
        Some(Seat::Peer("/run/magi/tau-mu.host".into()))
    );
    assert_eq!(app.step(true), Some(Seat::Own), "round to our own again");
    assert!(app.attached.is_none());
}

/// Backwards from home is the last agent, not nowhere.
#[test]
fn the_left_arrow_goes_the_other_way_round() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer"), peer("tau-mu", "scribe")]);
    assert_eq!(
        app.step(false),
        Some(Seat::Peer("/run/magi/tau-mu.host".into()))
    );
    assert_eq!(
        app.step(false),
        Some(Seat::Peer("/run/magi/beta-nu.host".into()))
    );
    assert_eq!(app.step(false), Some(Seat::Own));
}

/// **What melchior names is not what the arrows can reach.**
///
/// Its roster is everyone listening in the project — this session included, since it dials its own
/// socket like any other — and an agent that published no `.ui` note has a name and nowhere to
/// look. Counting those would draw a control that moves between one agent and itself.
#[test]
fn a_peer_with_no_screen_and_our_own_name_are_not_places_to_go() {
    let mut app = app_with(vec![
        Peer {
            id: "alpha-rho".to_owned(),
            role: "main".to_owned(),
            ui: Some("/run/magi/alpha-rho.host".into()),
        },
        Peer {
            id: "iota-mu".to_owned(),
            role: "worker".to_owned(),
            ui: None,
        },
    ]);
    assert_eq!(app.crew_size(), 1, "nowhere to go");
    assert_eq!(app.step(true), None);
    assert!(app.attached.is_none(), "and nothing moved");
}

/// A peer that went away while you were pointed at it puts the ring back at its start, so the
/// next arrow is the first agent rather than nothing.
#[test]
fn stepping_on_from_somebody_who_has_gone_starts_the_ring_again() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer")]);
    app.attach_to(Some(peer("psi-chi", "gone")));
    assert_eq!(
        app.step(true),
        Some(Seat::Peer("/run/magi/beta-nu.host".into()))
    );
}

/// The footer's name, and the only signal on screen that says whose session this is.
#[test]
fn a_peer_is_named_by_its_role_and_id() {
    let mut app = app_with(vec![]);
    assert_eq!(app.viewing(), "axum/main/alpha-rho");
    app.attach_to(Some(peer("beta-nu", "reviewer")));
    assert_eq!(app.viewing(), "reviewer/beta-nu");
    app.attach_to(Some(Peer {
        id: "tau-mu".to_owned(),
        role: String::new(),
        ui: Some("/run/magi/tau-mu.host".into()),
    }));
    assert_eq!(app.viewing(), "tau-mu", "an older melchior says no role");
}

/// **Peeking is reading.** Everything that would change a session is refused in front of the
/// socket, because the session on the other end cannot tell which of its UIs lives there.
#[test]
fn only_attaching_and_detaching_may_be_sent_to_somebody_else() {
    let reads = [
        UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
        },
        UiCommand::Detach,
    ];
    for command in reads {
        assert!(!drives(&command), "{command:?}");
    }
    let writes = [
        UiCommand::SubmitPrompt {
            text: "hello".into(),
            aside: String::new(),
        },
        UiCommand::Interrupt,
        UiCommand::SetModel { name: "x".into() },
        UiCommand::SetThinking {
            level: "off".into(),
        },
        UiCommand::Branch { keeps: Some(0) },
        UiCommand::TakeGrants { grants: Vec::new() },
        UiCommand::DeclareNeeds,
        UiCommand::Resume { id: "s".into() },
        UiCommand::Sized {
            rows: None,
            cols: 80,
            holds: false,
        },
    ];
    for command in writes {
        assert!(drives(&command), "{command:?}");
    }
}

/// And a refusal a person reads is not the same as one the machinery earns.
#[test]
fn a_width_is_refused_without_a_sentence_about_it() {
    assert!(spoken(&UiCommand::Interrupt));
    assert!(!spoken(&UiCommand::Sized {
        rows: None,
        cols: 80,
        holds: false,
    }));
}

/// The sentence itself: who you are reading, how to get back, and only once.
#[test]
fn the_refusal_says_what_to_do_about_it_and_says_it_once() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer")]);
    app.attach_to(Some(peer("beta-nu", "reviewer")));
    app.refuse_drive();
    app.refuse_drive();
    app.refuse_drive();
    assert_eq!(app.entries().len(), 1, "{:?}", app.entries());
    match &app.entries()[0] {
        Entry::Notice { text } => {
            assert!(text.contains("reviewer/beta-nu"), "{text}");
            assert!(text.contains("axum/main/alpha-rho"), "{text}");
            assert!(text.contains("alt+,"), "{text}");
        }
        other => panic!("expected a notice, got {other:?}"),
    }
}

/// Nothing is refused on our own session, which is the whole point of the gate being one-sided.
#[test]
fn our_own_session_is_never_refused_anything() {
    let mut app = app_with(vec![peer("beta-nu", "reviewer")]);
    app.refuse_drive();
    assert!(app.entries().is_empty(), "{:?}", app.entries());
}
