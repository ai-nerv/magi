//! Opening the info pane, and what each view puts in it.
//!
//! Split out under THE RULE; the app these act on is next door.

use super::*;

#[test]
fn the_trace_is_recorded_before_anybody_asks_for_it() {
    // **The property that makes `:trace` worth having.** A trace that began when it was opened
    // would be empty exactly when somebody went looking — which is always just after the thing
    // they wanted to see.
    let mut app = App::new();
    assert!(app.timeline.is_empty(), "nothing has happened yet");

    app.apply(HarnessEvent::ToolCallStarted {
        cursor: magi_proto::Cursor::ZERO,
        id: magi_proto::ToolCallId::new("t1".to_owned()),
        name: "bash".to_owned(),
        args: "{}".to_owned(),
    });
    assert_eq!(app.timeline.len(), 1, "recorded without being asked");
    assert!(app.pane.is_none(), "and nothing was opened");
}

#[test]
fn opening_the_trace_shows_what_was_recorded() {
    let mut app = App::new();
    app.apply(HarnessEvent::ToolCallStarted {
        cursor: magi_proto::Cursor::ZERO,
        id: magi_proto::ToolCallId::new("t1".to_owned()),
        name: "ripgrep".to_owned(),
        args: "{}".to_owned(),
    });
    app.show_trace();
    let pane = app.pane.expect("a pane");
    assert_eq!(pane.title, "trace");
    assert!(pane.follow, "opens at the newest end");
    assert!(
        pane.showing(10)[0].to_string().contains("ripgrep"),
        "{:?}",
        pane.showing(10)[0].to_string()
    );
}

#[test]
fn an_empty_trace_says_so_rather_than_drawing_an_empty_box() {
    let mut app = App::new();
    app.show_trace();
    let pane = app.pane.expect("a pane");
    assert_eq!(pane.showing(10)[0].to_string(), "nothing has happened yet");
}

#[test]
fn the_pane_is_not_the_menu_slot() {
    // The separation this design turns on: a tool holding rows and a person opening a view are
    // different things in different places, and neither closes the other.
    let mut app = App::new();
    app.show_trace();
    assert!(app.pane.is_some());
    assert!(
        app.overlay.is_none(),
        "opening a view did not touch the slot pickers, completions and tool surfaces share"
    );
}

#[test]
fn the_cost_view_says_nothing_was_spent_before_any_turn_finished() {
    let mut app = App::new();
    app.show_cost();
    let pane = app.pane.expect("a pane");
    assert_eq!(pane.title, "cost");
    assert!(
        pane.showing(10)[0].to_string().contains("nothing spent"),
        "{:?}",
        pane.showing(10)[0].to_string()
    );
}

#[test]
fn the_cost_view_counts_the_turns_that_finished() {
    let mut app = App::new();
    for (input, output) in [(100_u64, 20_u64), (200, 30)] {
        app.entries.push(Entry::Assistant {
            id: magi_proto::MessageId::new("a".to_owned()),
            text: "said".to_owned(),
            thinking: String::new(),
            stop_reason: Some(magi_proto::StopReason::EndTurn),
            usage: magi_proto::Usage {
                input,
                output,
                cache_read: 0,
                cache_write: 0,
            },
            error: None,
            signatures: magi_proto::Signatures::default(),
        });
    }
    app.show_cost();
    let said = app
        .pane
        .expect("a pane")
        .showing(40)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(said.contains("300"), "the total is the sum: {said}");
    assert!(!said.contains('$'), "and it invents no price: {said}");
}

#[test]
fn pressing_the_corner_a_second_time_closes_what_it_opened() {
    // A control that only ever opens is one you reach for the keyboard to undo, which is the
    // opposite of why it is a button.
    let mut app = App::new();
    app.press_corner();
    assert_eq!(
        app.pane.as_ref().map(|p| p.title.clone()).as_deref(),
        Some("cost")
    );
    app.press_corner();
    assert!(app.pane.is_none(), "the second press closed it");
    app.press_corner();
    assert!(app.pane.is_some(), "and the third opened it again");
}

#[test]
fn pressing_the_corner_over_another_view_shows_the_corners_own() {
    // Closing only when *its* view is up. Pressing the corner while the trace is open should
    // get you the corner's view, not nothing — the press means "show me this", and it does.
    let mut app = App::new();
    app.show_trace();
    app.press_corner();
    assert_eq!(
        app.pane.as_ref().map(|p| p.title.clone()).as_deref(),
        Some("cost"),
        "the corner's own view, not a dismissal"
    );
}

#[test]
fn the_corner_draws_and_opens_the_same_thing() {
    // A corner that showed one thing and opened another would be a button that lies about
    // itself: you press what you were reading, so what you were reading is what you get.
    let corner = magi_tui::corner::Corner::default();
    let mut app = App::new();
    app.corner = corner;
    app.press_corner();
    assert_eq!(
        app.pane.as_ref().map(|p| p.title.clone()).as_deref(),
        Some(corner.opens())
    );
}
