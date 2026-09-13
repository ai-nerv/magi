//! The footer name: the button that opens the agents view, and how it is drawn. The `< >` control
//! that used to move between agents is gone — the agents view took that over — so the name sits
//! against the left edge whatever the crew, and inverts while the pointer is on it.

use super::*;
use ratatui::style::Modifier;

fn row(width: u16, crew: usize) -> String {
    let data = FooterData {
        input_tokens: 12_500,
        output_tokens: 900,
        context_percent: Some(6.2),
        context_window: 200_000,
        identity: "axum/main/alpha".into(),
        model: "claude-opus-5".into(),
        crew,
        own: true,
        name_hover: false,
    };
    render(&data, &[Span::raw("waiting")], width)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

#[test]
fn the_name_is_against_the_left_edge_whatever_the_crew() {
    for crew in [1usize, 2, 9] {
        assert!(
            row(80, crew).trim_start().starts_with("axum/main/alpha"),
            "crew {crew}"
        );
    }
}

#[test]
fn there_is_no_crew_control_any_more() {
    // It used to draw `< >` at the left when there was somewhere to go; the agents view replaced it.
    assert!(!row(80, 3).contains("< >"), "{}", row(80, 3));
}

#[test]
fn the_row_is_still_the_width_it_was_given() {
    for width in 30..90u16 {
        for crew in [1usize, 2, 9] {
            let line = row(width, crew);
            assert_eq!(
                line.chars().count(),
                usize::from(width),
                "width {width}, crew {crew}: {line:?}"
            );
        }
    }
}

#[test]
fn the_name_stays_inside_the_inset() {
    let pad = usize::from(crate::metric::footer_pad());
    let head: String = row(80, 3).chars().take(pad).collect();
    assert!(head.trim().is_empty(), "the left end: {}", row(80, 3));
}

fn name_style(width: u16, hover: bool) -> Style {
    let data = FooterData {
        identity: "axum/main/alpha".into(),
        model: "claude-opus-5".into(),
        name_hover: hover,
        ..FooterData::default()
    };
    render(&data, &[], width)[0]
        .spans
        .iter()
        .find(|s| s.content.contains("alpha"))
        .expect("the name")
        .style
}

#[test]
fn a_peers_name_is_styled_apart_from_your_own() {
    let styled = |own: bool| {
        let data = FooterData {
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            own,
            ..FooterData::default()
        };
        render(&data, &[], 80)[0]
            .spans
            .iter()
            .find(|s| s.content.contains("alpha"))
            .expect("the name")
            .style
    };
    assert_ne!(styled(true), styled(false), "a peer reads as your own");
}

#[test]
fn the_name_inverts_under_the_pointer_and_not_otherwise() {
    // The invert is how the name says it is a button, the same block the usage badge always wears.
    assert!(
        !name_style(80, false)
            .add_modifier
            .contains(Modifier::REVERSED),
        "the name should not be inverted at rest"
    );
    assert!(
        name_style(80, true)
            .add_modifier
            .contains(Modifier::REVERSED),
        "the name should invert while hovered"
    );
}

#[test]
fn where_the_name_lands_is_where_it_is_drawn() {
    // The recorded click target must cover the drawn name, or a click misses the button it is on.
    let data = FooterData {
        identity: "axum/main/alpha".into(),
        model: "claude-opus-5".into(),
        ..FooterData::default()
    };
    let columns = name_columns(&data, 80);
    let line: String = render(&data, &[], 80)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let at = line.find("axum").expect("the name is drawn");
    let start = usize::from(columns.start);
    assert_eq!(line[..at].chars().count(), start, "start column: {line:?}");
    assert!(columns.end > columns.start, "a real span: {columns:?}");
}
