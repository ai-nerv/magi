//! The shape of a block: a header inside its fill, and a fill that closes it.

use super::*;

const FILL: Color = Color::Indexed(237);

fn head() -> Head {
    Head {
        label: "shell".into(),
        mark: Some((glyph::outcome_ok(), Color::Green)),
        fold: Some(Detail::Preview),
        copy: true,
    }
}

fn text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// The colour behind each column of `line`.
fn grounds(line: &Line<'_>) -> Vec<Option<Color>> {
    line.spans
        .iter()
        .flat_map(|span| {
            std::iter::repeat_n(ground(span.style), crate::wrap::columns(&span.content))
        })
        .collect()
}

fn with_rows(rows: usize, width: u16) -> Vec<Line<'static>> {
    let mut block = Block::new(head(), FILL);
    let style = block.style();
    block.rows = (0..rows)
        .map(|n| frame::inside(Line::from(format!("row {n}")), width, style, 4))
        .collect();
    block.lines(width)
}

#[test]
fn a_header_is_exactly_the_width_however_narrow() {
    for width in 6..=120u16 {
        let line = header(&head(), FILL, width);
        assert_eq!(
            crate::wrap::columns(&text(&line)),
            usize::from(width),
            "at {width}: {:?}",
            text(&line)
        );
    }
}

#[test]
fn the_header_sits_inside_the_fill() {
    let line = header(&head(), FILL, 60);
    let cells = grounds(&line);
    assert_eq!(cells[0], None, "the first column is the screen's");
    assert_eq!(cells[59], None, "and the last");
    assert!(
        cells[1..59].iter().all(|cell| *cell == Some(FILL)),
        "{cells:?}"
    );
}

#[test]
fn the_rule_and_the_label_are_drawn_in_the_screens_own_colour() {
    let line = header(&head(), FILL, 60);
    for span in &line.spans {
        if span.content.contains(glyph::block_edge()) || span.content.contains("shell") {
            assert!(
                span.style.add_modifier.contains(Modifier::REVERSED),
                "{:?}",
                span.content
            );
            assert_eq!(span.style.fg, Some(FILL), "{:?}", span.content);
        }
    }
}

#[test]
fn the_mark_keeps_its_own_colour() {
    let line = header(&head(), FILL, 60);
    let mark = line
        .spans
        .iter()
        .find(|span| span.content.contains(glyph::outcome_ok()))
        .expect("the mark");
    assert_eq!(mark.style.fg, Some(Color::Green));
    assert_eq!(mark.style.bg, Some(FILL));
}

#[test]
fn the_handle_is_the_last_thing_given_up() {
    let shown = text(&header(&head(), FILL, 22));
    assert!(
        shown.contains(&format!("[ {} ]", glyph::expand())),
        "{shown:?}"
    );
    assert!(!shown.contains(glyph::copy()), "{shown:?}");
}

#[test]
fn a_long_label_is_cut_rather_than_pushing_the_handle_off() {
    let mut long = head();
    long.label = "a-tool-with-a-very-long-name".into();
    let shown = text(&header(&long, FILL, 30));
    assert!(shown.contains(glyph::ellipsis()), "{shown:?}");
    assert!(
        shown.contains(&format!("[ {} ]", glyph::expand())),
        "{shown:?}"
    );
}

#[test]
fn a_block_with_nothing_inside_is_its_header_alone() {
    let lines = with_rows(0, 40);
    assert_eq!(lines.len(), 1);
    assert!(is_header(&lines[0]));
}

#[test]
fn a_block_is_one_rectangle_of_its_fill() {
    let lines = with_rows(2, 40);
    assert_eq!(lines.len(), 5, "header, fill, two rows, fill");
    for line in &lines {
        let cells = grounds(line);
        assert_eq!(cells.len(), 40);
        assert_eq!(cells[0], None);
        assert_eq!(cells[39], None);
        assert!(
            cells[1..39].iter().all(|cell| *cell == Some(FILL)),
            "{cells:?}"
        );
    }
}

#[test]
fn only_the_header_reads_as_one() {
    let lines = with_rows(1, 40);
    assert!(is_header(&lines[0]));
    assert!(lines[1..].iter().all(|line| !is_header(line)));
    let seam = frame::rule(40, Style::default().bg(FILL));
    assert!(!is_header(&seam));
}
