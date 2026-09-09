//! The control for moving between agents, and the width it is allowed to cost.

use super::*;

fn row(width: u16, crew: usize, own: bool) -> String {
    let data = FooterData {
        input_tokens: 12_500,
        output_tokens: 900,
        context_percent: Some(6.2),
        context_window: 200_000,
        identity: "axum/main/alpha".into(),
        model: "claude-opus-5".into(),
        crew,
        own,
    };
    render(&data, &[Span::raw("waiting")], width)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

#[test]
fn a_crew_of_one_draws_no_control() {
    let line = row(80, 1, true);
    assert!(!line.contains(CREW), "{line:?}");
    assert!(line.trim_start().starts_with("axum/main/alpha"), "{line:?}");
}

#[test]
fn a_crew_of_more_than_one_draws_it_left_of_the_name() {
    let line = row(80, 3, true);
    let trimmed = line.trim_start();
    assert!(trimmed.starts_with(CREW), "{line:?}");
    assert!(
        trimmed.find(CREW) < trimmed.find("axum/main/alpha"),
        "the control must come before the name: {line:?}"
    );
}

/// Charging the arrows to the name's budget but not to the middle's floor makes the middle
/// disappear at widths where the floor falls in the gap, with nothing looking broken. It shows
/// only with a middle long enough that its natural centre falls left of the control: forty
/// characters and a crew, at width 82.
#[test]
fn the_control_does_not_cost_the_middle_its_place() {
    let long = "x".repeat(40);
    let data = FooterData {
        input_tokens: 0,
        output_tokens: 0,
        context_percent: None,
        context_window: 0,
        identity: "axum/main/alpha".into(),
        model: "claude-opus-5".into(),
        crew: 4,
        own: true,
    };
    let line: String = render(&data, &[Span::raw(long.clone())], 82)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        line.contains(&long),
        "the middle was dropped where it fits: {line:?}"
    );
}

#[test]
fn the_middle_never_prints_into_the_control() {
    for width in 30..90u16 {
        let line = row(width, 4, true);
        let Some(at) = line.find(CREW) else {
            continue;
        };
        let after: String = line.chars().skip(at + CREW.chars().count()).collect();
        assert!(
            after.starts_with(' '),
            "width {width}: something is against the control: {line:?}"
        );
    }
}

#[test]
fn the_row_is_still_the_width_it_was_given() {
    for width in 30..90u16 {
        for crew in [1usize, 2, 9] {
            let line = row(width, crew, true);
            assert_eq!(
                line.chars().count(),
                usize::from(width),
                "width {width}, crew {crew}: {line:?}"
            );
        }
    }
}

#[test]
fn the_control_stays_inside_the_inset() {
    let pad = usize::from(crate::metric::footer_pad());
    let line = row(80, 3, true);
    let head: String = line.chars().take(pad).collect();
    assert!(head.trim().is_empty(), "the left end: {line:?}");
}

#[test]
fn a_peers_name_is_styled_apart_from_your_own() {
    let data = |own: bool| FooterData {
        input_tokens: 0,
        output_tokens: 0,
        context_percent: None,
        context_window: 0,
        identity: "axum/main/alpha".into(),
        model: "claude-opus-5".into(),
        crew: 2,
        own,
    };
    let styled = |own: bool| {
        render(&data(own), &[], 80)[0]
            .spans
            .iter()
            .find(|s| s.content.contains("alpha"))
            .expect("the name")
            .style
    };
    assert_ne!(styled(true), styled(false), "a peer reads as your own");
}
