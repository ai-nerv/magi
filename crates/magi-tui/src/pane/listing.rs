//! A float holding a list: a cursor that moves entry by entry, and the band that shows it.
//!
//! Split out under THE RULE; the panel these drive is next door.

use super::*;

/// A list of `entries` targets, `tall` rows each, under a count line and a blank.
fn list(entries: usize, tall: usize) -> Pane {
    let mut rows = vec![Line::from("count"), Line::from("")];
    let mut picks = vec![None, None];
    for nth in 0..entries {
        for row in 0..tall {
            rows.push(Line::from(format!("entry {nth} row {row}")));
            picks.push(Some(format!("e{nth}")));
        }
    }
    Pane::new("agents", rows).selectable(picks)
}

#[test]
fn the_keys_move_from_entry_to_entry_not_row_to_row() {
    let mut pane = list(3, 3);
    pane.first();
    assert_eq!(pane.chosen(), Some("e0"));
    pane.step(true);
    assert_eq!(pane.chosen(), Some("e1"));
    pane.step(true);
    pane.step(true);
    assert_eq!(pane.chosen(), Some("e2"), "and stops at the last");
    pane.step(false);
    assert_eq!(pane.chosen(), Some("e1"));
}

#[test]
fn the_whole_entry_is_lit_and_nothing_else() {
    let mut pane = list(2, 3);
    pane.point_at("e1");
    let shown = pane.showing(20);
    let lit: Vec<bool> = shown
        .iter()
        .map(|line| line.to_string().starts_with(GUTTER))
        .collect();
    assert_eq!(lit, [false, false, false, false, false, true, true, true]);
}

#[test]
fn every_row_of_a_list_keeps_one_column() {
    // Lit or not, the gutter is two cells, so an entry does not jump sideways under the cursor.
    let mut pane = list(2, 2);
    pane.point_at("e0");
    for line in pane.showing(20) {
        let text = line.to_string();
        assert!(
            text.starts_with(GUTTER) || text.starts_with(NO_GUTTER),
            "{text:?}"
        );
    }
}

#[test]
fn the_pointer_moves_the_cursor_and_leaving_does_not_lose_it() {
    let mut pane = list(3, 3);
    pane.first();
    // Row 3 of the panel is the first content row; the third entry starts eight rows further.
    assert!(pane.hover_at(HEAD + 8), "onto the third entry");
    assert_eq!(pane.chosen(), Some("e2"));
    assert!(
        !pane.hover_at(HEAD + 9),
        "a row further within it is no change"
    );
    assert!(!pane.hover_at(0), "the frame selects nothing");
    assert_eq!(pane.chosen(), Some("e2"), "and the cursor stayed put");
}

#[test]
fn the_cursor_is_brought_into_view() {
    let mut pane = list(20, 3);
    pane.last();
    pane.settle(10);
    assert!(pane.top > 0, "scrolled to the last entry");
    assert!(pane.top + 10 >= pane.rows.len(), "all of it shown");
    pane.first();
    pane.settle(10);
    assert_eq!(pane.top, 0, "back up to the heading of the list");
}

#[test]
fn a_band_reaches_the_frame() {
    // The band is behind the whole row, not just the text on it: the padding wears it too.
    let mut pane = list(1, 1);
    pane.point_at("e0");
    let drawn = pane.framed(40, 5, 0, crate::border::Scan::Off);
    let row = &drawn[HEAD as usize + 2];
    let band = Some(crate::colour::pane_selected_bg());
    let pad = &row.spans[row.spans.len() - 2];
    assert_eq!(pad.style.bg, band, "{row:?}");
}

#[test]
fn a_row_too_long_for_the_panel_is_cut_rather_than_breaking_the_frame() {
    let pane = Pane::new("cost", vec![Line::from("x".repeat(500))]);
    for line in pane.framed(40, 5, 0, crate::border::Scan::Off) {
        assert_eq!(line.to_string().chars().count(), 40, "{line}");
    }
}
