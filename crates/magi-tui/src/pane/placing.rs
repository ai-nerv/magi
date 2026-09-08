//! Where a float sits, and what it shows.
//!
//! Split out under THE RULE; the panel these place is next door.

use super::*;

fn screen(width: u16, height: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width,
        height,
    }
}

fn rows(n: usize) -> Vec<Line<'static>> {
    (0..n).map(|i| Line::from(format!("row {i}"))).collect()
}

#[test]
fn it_sits_in_the_middle_with_the_same_margin_on_both_sides() {
    // Centred by construction rather than by arithmetic at the call site, so an odd number of
    // spare columns leans one way consistently instead of drifting as the window is resized.
    let area = Pane::area(screen(100, 40));
    let left = area.x;
    let right = 100 - (area.x + area.width);
    assert!(left.abs_diff(right) <= 1, "left {left}, right {right}");

    let top = area.y;
    let bottom = 40 - (area.y + area.height);
    assert!(top.abs_diff(bottom) <= 1, "top {top}, bottom {bottom}");
}

#[test]
fn it_leaves_the_conversation_visible_around_it() {
    // A float that covered the screen would be a screen, and a screen is a thing you navigate
    // rather than glance at.
    let area = Pane::area(screen(100, 40));
    assert!(area.width < 100, "{area:?}");
    assert!(area.height < 40, "{area:?}");
}

#[test]
fn a_screen_too_small_to_inset_still_produces_a_drawable_area() {
    // Terminals get resized to silly sizes mid-session. Every one of these must be a rectangle
    // that fits on the screen it was measured against, rather than a panic or a negative width.
    for (w, h) in [(1, 1), (4, 2), (10, 3), (0, 0)] {
        let area = Pane::area(screen(w, h));
        assert!(area.x + area.width <= w, "{w}x{h} -> {area:?}");
        assert!(area.y + area.height <= h, "{w}x{h} -> {area:?}");
    }
}

#[test]
fn nothing_to_show_says_so_rather_than_drawing_an_empty_box() {
    // An empty view usually wants to say how to make it non-empty, and a blank rectangle says
    // nothing at all.
    let float = Pane::new("graph", Vec::new()).saying("run `:graph init` first");
    let shown = float.showing(10);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].to_string(), "run `:graph init` first");
}

#[test]
fn scrolling_stops_at_the_last_page_rather_than_past_it() {
    // Scrolling past the end is how a list ends up showing an empty box: the content is above
    // the viewport and there is nothing on screen to say so.
    let mut float = Pane::new("trace", rows(50));
    float.down(1_000, 10);
    assert_eq!(float.top, 40);
    assert_eq!(float.showing(10).len(), 10);
    assert_eq!(float.showing(10)[0].to_string(), "row 40");
}

#[test]
fn a_float_shorter_than_its_page_does_not_scroll_at_all() {
    let mut float = Pane::new("trace", rows(3));
    float.down(5, 10);
    assert_eq!(float.top, 0);
    assert_eq!(float.showing(10).len(), 3);
}

#[test]
fn it_says_where_in_the_list_you_are_when_there_is_more_than_fits() {
    // A float that silently holds four hundred rows and shows twenty looks like a float that
    // holds twenty.
    let mut float = Pane::new("trace", rows(400));
    assert_eq!(float.more(20).as_deref(), Some("1–20 of 400"));
    float.down(10, 20);
    assert_eq!(float.more(20).as_deref(), Some("11–30 of 400"));
    // And says nothing when everything fits, rather than "1–3 of 3".
    assert_eq!(Pane::new("t", rows(3)).more(20), None);
}

#[test]
fn the_bottom_is_the_newest_end() {
    // What a trace wants on open: the last thing that happened, not the first.
    let mut float = Pane::new("trace", rows(50));
    float.bottom(10);
    assert_eq!(float.showing(10)[0].to_string(), "row 40");
}
