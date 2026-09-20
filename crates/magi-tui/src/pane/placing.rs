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
    let float = Pane::new("trace", Vec::new()).saying("nothing has happened yet");
    let shown = float.showing(10);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].to_string(), "nothing has happened yet");
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

#[test]
fn the_frame_is_the_same_ring_the_prompt_box_wears() {
    // Not a box drawn by hand beside another box: the same `border` module, so the two are
    // parts of one program rather than two people's idea of a rounded corner.
    let pane = Pane::new("trace", rows(3));
    let drawn = pane.framed(40, 10, 0, crate::border::Scan::Resting);
    let text: Vec<String> = drawn.iter().map(ToString::to_string).collect();
    assert!(
        text[0].starts_with(crate::glyph::corner_top_left()),
        "{text:?}"
    );
    assert!(
        text[text.len() - 1].starts_with(crate::glyph::corner_bottom_left()),
        "{text:?}"
    );
}

#[test]
fn every_row_of_the_frame_is_the_width_it_was_given() {
    // A short row leaves the border ragged; a long one pushes it off the end. Both read as a
    // broken box rather than a full one.
    for width in [24_u16, 40, 80] {
        let pane = Pane::new("cost", rows(4));
        for line in pane.framed(width, 10, 3, crate::border::Scan::Resting) {
            assert_eq!(
                line.to_string().chars().count(),
                usize::from(width),
                "at {width}: {line}"
            );
        }
    }
}

#[test]
fn the_heading_is_inside_the_frame_rather_than_in_the_rule() {
    // A border title is drawn *in* the line, which forces the frame to break for the word — and
    // a box with a gap in it reads as damaged rather than labelled.
    let pane = Pane::new("trace", rows(2));
    let drawn = pane.framed(40, 10, 0, crate::border::Scan::Off);
    assert!(!drawn[0].to_string().contains("trace"), "not in the rule");
    assert!(drawn[1].to_string().contains("trace"), "in the first row");
}

#[test]
fn the_scan_moves_with_the_tick() {
    // The light travels. Two frames one tick apart must differ somewhere on the border, or the
    // pane is wearing a still picture of an animation.
    let pane = Pane::new("trace", rows(6));
    let a = pane.framed(40, 10, 0, crate::border::Scan::Resting);
    let b = pane.framed(40, 10, 40, crate::border::Scan::Resting);
    assert_ne!(
        format!(
            "{:?}",
            a.iter().map(|l| l.spans.clone()).collect::<Vec<_>>()
        ),
        format!(
            "{:?}",
            b.iter().map(|l| l.spans.clone()).collect::<Vec<_>>()
        ),
        "the border did not move between ticks"
    );
}

#[test]
fn the_window_is_the_same_size_whatever_it_holds() {
    // A window that shrank to its contents would jump every time you scrolled it or opened a
    // different view, and a box that moves under you is one you have to find again each time.
    let empty = Pane::new("cost", Vec::new()).saying("nothing yet");
    let full = Pane::new("trace", rows(500));
    let (page, width) = (18, 60);
    assert_eq!(
        empty.framed(width, page, 0, crate::border::Scan::Off).len(),
        full.framed(width, page, 0, crate::border::Scan::Off).len(),
        "one row of content and five hundred draw the same box"
    );
    assert_eq!(
        empty.framed(width, page, 0, crate::border::Scan::Off).len(),
        page + 4,
        "the page, the heading, the blank under it, and two border rows"
    );
}
