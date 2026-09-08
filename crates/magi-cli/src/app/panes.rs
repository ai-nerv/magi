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
fn the_graph_says_how_to_fill_itself() {
    // Nothing indexes the tree yet. Saying "no index" would name the problem; naming the command
    // says what to do about it.
    let mut app = App::new();
    app.show_graph(":graph", 100);
    let pane = app.pane.expect("a pane");
    assert_eq!(pane.title, "graph");
    assert!(
        pane.showing(10)[0].to_string().contains(":graph init"),
        "{:?}",
        pane.showing(10)[0].to_string()
    );
}

#[test]
fn graph_init_says_it_is_not_built_rather_than_doing_nothing() {
    // A command that silently did nothing is indistinguishable from an index that found nothing,
    // and the second is a much worse thing to believe.
    let mut app = App::new();
    app.show_graph(":graph init", 100);
    assert!(app.pane.is_none(), "init does not open the view");
    let said = app
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<String>();
    assert!(said.contains("not built yet"), "{said}");
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
