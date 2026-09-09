//! The prompt, as Pi draws it: a horizontal rule above and below, no gutter, text from column zero.
//! The cursor is drawn into the line with inverse video rather than parked with the terminal's own,
//! which is what lets the block scroll and wrap without the cursor drifting off it.

#[cfg(test)]
#[path = "prompt/says.rs"]
mod says_tests;

use crate::colour;
use crate::editor::Editor;
use crate::glyph;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// How many text rows the prompt will show on a terminal `rows` tall.
#[must_use]
pub fn visible_rows(rows: u16) -> usize {
    usize::from(crate::metric::share(rows, crate::metric::prompt_share()))
        .max(usize::from(crate::metric::prompt_min_lines()))
}

/// The blank prompt: the cursor, then this session's placeholder, dimmer than the text so it does
/// not read as something already in the box.
pub(crate) fn placeholder_spans(
    width: u16,
    saying: &crate::tease::Saying<'_>,
) -> Vec<Span<'static>> {
    // A placeholder cut in half looks broken, so a very narrow screen gets nothing.
    let hint = if saying.text.chars().count() < usize::from(width) {
        saying.text
    } else if glyph::placeholder_short().chars().count() < usize::from(width) {
        glyph::placeholder_short()
    } else {
        ""
    };

    let dim = Style::default().fg(colour::hint());
    // Your own cursor, on the first letter rather than in front of it. Painted only in normal mode:
    // in insert mode the terminal draws an underline, and a block over it would say the other mode.
    let mine = if saying.mode.is_insert() {
        dim
    } else {
        Style::default()
            .fg(colour::text())
            .add_modifier(Modifier::REVERSED)
    };
    // The ghost, where the box is editing itself, dimmer than yours. Always one of two shapes: a
    // block over the character it is on, an underline while it is typing — a bar belongs between
    // two cells and a cell grid has no between.
    let ghost = if saying.block {
        dim.add_modifier(Modifier::REVERSED)
    } else {
        dim.add_modifier(Modifier::UNDERLINED)
    };
    // What it is about to take out, in the same inversion as the block.
    let marked = dim.add_modifier(Modifier::REVERSED);

    let mut spans = Vec::new();
    let letters: Vec<char> = hint.chars().collect();
    if letters.is_empty() {
        return vec![Span::styled(" ", mine)];
    }
    for (at, letter) in letters.iter().enumerate() {
        let style = if saying
            .marked
            .as_ref()
            .is_some_and(|span| span.contains(&at))
        {
            marked
        } else if saying.caret == Some(at) {
            ghost
        } else if at == 0 {
            mine
        } else {
            dim
        };
        spans.push(Span::styled(letter.to_string(), style));
    }
    // Past the last letter, which is where a caret sits while text is being added to the end.
    if saying.caret == Some(letters.len()) {
        spans.push(Span::styled(" ", ghost));
    }
    spans
}

/// How many text rows the prompt shows right now, on a terminal `rows` tall. Worked out rather than
/// drawn and counted: the caller sizes the box before it knows how much room a menu has.
#[must_use]
pub fn text_rows(editor: &Editor, rows: u16, width: u16, badge: &str) -> usize {
    if editor.lines().len() == 1 && editor.lines()[0].is_empty() {
        return 1;
    }
    // Folded rows, not logical lines: a three-line prompt sized from logical lines gets one row.
    let (visual, caret, _) = crate::fold::fold_all(editor, crate::fold::text_room(width, badge));
    let total = visual.len().max(1);
    let max_visible = visible_rows(rows);
    let offset = caret.saturating_sub(max_visible.saturating_sub(1));
    let offset = offset.min(total.saturating_sub(max_visible.min(total)));
    (offset + max_visible).min(total) - offset
}

/// The prompt box, and where inside it the menu landed. The rows are needed to translate a click
/// into the menu's coordinates, and only the renderer knows where the divider ended up.
pub struct Boxed {
    pub lines: Vec<Line<'static>>,
    /// Which of those rows the menu occupies. Empty when nothing is open.
    pub menu: std::ops::Range<usize>,
    /// Where the usage badge landed: the row within the box, and the columns it covers. `None` when
    /// there is no badge. A click on it opens the cost view.
    pub badge: Option<(usize, std::ops::Range<u16>)>,
}

/// How far in from the left edge a menu row's text starts: the side, then a space.
pub const INSET: u16 = 2;

/// Render the prompt as a box, with `menu` inside it under a divider. Inside rather than beneath,
/// so the box is what says where the rows are; `menu` is already `width - 3` wide — see
/// [`crate::metric::gutter`] — and empty when nothing is open. `rows` is the terminal height, which
/// sets how much of a long prompt is shown; `tick` drives the scan and `scan` says what it is doing.
#[must_use]
pub fn render(
    editor: &Editor,
    width: u16,
    rows: u16,
    tick: usize,
    scan: crate::border::Scan,
    menu: &[Line<'static>],
    saying: crate::tease::Saying<'_>,
) -> Boxed {
    let badge = saying.badge;
    let text_style = Style::default().fg(colour::text());
    // What is left for text once the sides, the padding and the badge's strip are taken out.
    let room = crate::fold::text_room(width, badge);
    let blank = editor.lines().len() == 1 && editor.lines()[0].is_empty();

    // Folded first, then scrolled over the folded rows: scrolling logical lines and drawing folded
    // ones disagree about how far down the cursor is.
    let (visual, caret_row, caret_col) = crate::fold::fold_all(editor, room);
    let total_rows = visual.len().max(1);
    let max_visible = visible_rows(rows);
    let offset = caret_row.saturating_sub(max_visible.saturating_sub(1));
    let offset = offset.min(total_rows.saturating_sub(max_visible.min(total_rows)));
    let end = (offset + max_visible).min(total_rows);
    let shown = if blank { 1 } else { end - offset };
    // The divider is a content row, so the scan runs past it rather than round a hole in the box.
    let content = shown + if menu.is_empty() { 0 } else { 1 + menu.len() };
    let (top, bottom) = crate::border::edges(width, content, tick, scan);

    let mut out = Vec::with_capacity(content + 2);
    out.push(tagged(hidden(top, Direction::Up, offset), saying.mode));

    for row in 0..shown {
        let body = if blank {
            placeholder_spans(u16::try_from(room).unwrap_or(u16::MAX), &saying)
        } else {
            let index = offset + row;
            let text = visual.get(index).cloned().unwrap_or_default();
            if index == caret_row {
                with_cursor(&text, caret_col, text_style, saying.mode)
            } else {
                vec![Span::styled(text, text_style)]
            }
        };
        out.push(framed(
            body,
            width,
            content,
            row,
            tick,
            scan,
            &crate::fold::strip(badge, shown, row, saying.badge_open),
        ));
    }

    // The strip sits on the middle text row, one down for the top border, from `fold::strip`'s numbers.
    let worn = u16::try_from(badge.chars().count() + 3).unwrap_or(u16::MAX);
    let badge_at = (!badge.is_empty()).then(|| {
        let right = width.saturating_sub(1);
        (1 + shown / 2, right.saturating_sub(worn)..right)
    });

    let mut opened = 0..0;
    if !menu.is_empty() {
        out.push(divider(width, content, shown, tick, scan));
        opened = out.len()..out.len() + menu.len();
        for (row, line) in menu.iter().enumerate() {
            let at = shown + 1 + row;
            out.push(framed(
                line.spans.clone(),
                width,
                content,
                at,
                tick,
                scan,
                // No strip: the badge belongs to the box you type in, and a list under it is not that.
                &[],
            ));
        }
    }

    let below = total_rows.saturating_sub(end);
    out.push(hidden(bottom, Direction::Down, below));
    Boxed {
        lines: out,
        menu: opened,
        badge: badge_at,
    }
}

/// One content row between its two side bars.
fn framed(
    body: Vec<Span<'static>>,
    width: u16,
    content: usize,
    row: usize,
    tick: usize,
    scan: crate::border::Scan,
    tail: &[Span<'static>],
) -> Line<'static> {
    let (left, right) = crate::border::side(width, content, row, tick, scan);
    let worn: usize = tail.iter().map(|s| s.content.chars().count()).sum();
    let mut spans = vec![left, Span::raw(" ")];
    spans.extend(pad(
        body,
        width
            .saturating_sub(3)
            .saturating_sub(u16::try_from(worn).unwrap_or(0)),
    ));
    spans.extend(tail.iter().cloned());
    spans.push(right);
    Line::from(spans)
}

/// The rule between the text and the menu, teed into the sides so the box reads as one frame.
fn divider(
    width: u16,
    content: usize,
    row: usize,
    tick: usize,
    scan: crate::border::Scan,
) -> Line<'static> {
    let (left, right) = crate::border::side(width, content, row, tick, scan);
    let rule = glyph::edge_horizontal().repeat(usize::from(width.saturating_sub(2)));
    Line::from(vec![
        Span::styled(glyph::divider_left().to_owned(), left.style),
        Span::styled(rule, Style::default().fg(colour::border())),
        Span::styled(glyph::divider_right().to_owned(), right.style),
    ])
}

/// Pad a row out so the right-hand bar lands at the edge.
fn pad(mut spans: Vec<Span<'static>>, width: u16) -> Vec<Span<'static>> {
    // A folded row keeps the space it broke on, so it can stand one column past the width it was
    // folded at and shove the badge off the end.
    let mut used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    while used > usize::from(width) {
        let Some(last) = spans.last_mut() else { break };
        if !last.content.ends_with(' ') {
            break;
        }
        let mut text = last.content.to_string();
        text.pop();
        last.content = text.into();
        used -= 1;
    }
    let room = usize::from(width).saturating_sub(used);
    if room > 0 {
        spans.push(Span::raw(" ".repeat(room)));
    }
    spans
}

/// Write "N more" into an edge when the prompt is scrolled, on the border rather than instead of it.
fn hidden(edge: Line<'static>, direction: Direction, count: usize) -> Line<'static> {
    if count == 0 {
        return edge;
    }
    let arrow = match direction {
        Direction::Up => '↑',
        Direction::Down => '↓',
    };
    let label = format!(" {arrow} {count} more ");
    let width = columns_of(&edge);
    if label.chars().count() + 8 > width {
        return edge;
    }
    // Right on the top edge, left on the bottom: the mode has the top-left corner.
    let at = match direction {
        Direction::Up => width - 2 - label.chars().count(),
        Direction::Down => 2,
    };
    caption(edge, &label, Style::default().fg(colour::dim()), at)
}

/// Write the mode onto the top edge of the box, top-left, in three letters always so the frame does
/// not move when the mode does.
fn tagged(edge: Line<'static>, mode: crate::vim::Mode) -> Line<'static> {
    let label = format!(" {} ", mode.tag());
    if label.chars().count() + 4 > columns_of(&edge) {
        return edge;
    }
    // Insert mode is the one where a keystroke changes something.
    let style = Style::default().fg(if mode.is_insert() {
        colour::accent()
    } else {
        colour::border()
    });
    caption(edge, &label, style, 2)
}

/// How many columns an edge occupies.
fn columns_of(edge: &Line<'static>) -> usize {
    edge.spans.iter().map(|s| s.content.chars().count()).sum()
}

/// Write `label` over the columns it covers rather than spliced between spans: an edge's span
/// boundaries move as the scan travels along it, so a span index cuts somewhere different a frame.
fn caption(edge: Line<'static>, label: &str, style: Style, at: usize) -> Line<'static> {
    let mut columns: Vec<Span<'static>> = edge
        .spans
        .into_iter()
        .flat_map(|span| {
            let style = span.style;
            span.content
                .chars()
                .map(|c| Span::styled(c.to_string(), style))
                .collect::<Vec<_>>()
        })
        .collect();
    for (index, c) in label.chars().enumerate() {
        if let Some(column) = columns.get_mut(at + index) {
            *column = Span::styled(c.to_string(), style);
        }
    }
    Line::from(columns)
}

enum Direction {
    Up,
    Down,
}

/// Draw one line, with the cursor cell inverted in normal mode only — insert mode puts the cursor
/// between two characters, so there the terminal's own bar does it and [`crate::vim::Mode`] is what
/// the caller sets its shape from. At the end of a line a space is added and inverted, which is why
/// the layout reserves a column for it.
fn with_cursor(text: &str, col: usize, style: Style, mode: crate::vim::Mode) -> Vec<Span<'static>> {
    if mode.is_insert() {
        return vec![Span::styled(text.to_owned(), style)];
    }
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let inverted = style.add_modifier(Modifier::REVERSED);

    let before: String = chars[..col].iter().collect();
    let mut spans = Vec::with_capacity(3);
    if !before.is_empty() {
        spans.push(Span::styled(before, style));
    }

    match chars.get(col) {
        Some(&c) => {
            spans.push(Span::styled(c.to_string(), inverted));
            let after: String = chars[col + 1..].iter().collect();
            if !after.is_empty() {
                spans.push(Span::styled(after, style));
            }
        }
        None => spans.push(Span::styled(" ", inverted)),
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with(text: &str) -> Editor {
        let mut e = Editor::new();
        e.insert_str(text);
        e
    }

    fn rows_of(drawn: &Boxed) -> Vec<String> {
        drawn
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn an_empty_prompt_says_what_to_do_with_it() {
        // An empty box between two rules gives no way to tell a waiting prompt from a hung screen.
        let rendered = rows_of(&render(
            &Editor::new(),
            40,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying {
                text: "what are we making?",
                caret: None,
                badge_open: false,
                badge: "",
                mode: crate::vim::Mode::default(),
                block: false,
                marked: None,
            },
        ));
        assert_eq!(rendered.len(), 3, "top edge, text, bottom edge");
        let said = rendered[1].trim().trim_matches('│').trim();
        assert_eq!(said, "what are we making?");
    }

    #[test]
    fn one_keystroke_replaces_the_hint_with_the_text() {
        let rendered = rows_of(&render(
            &editor_with("h"),
            40,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying::default(),
        ));
        assert!(!rendered[1].contains("commands"), "{:?}", rendered[1]);
        assert!(
            rendered[1].starts_with("│ h"),
            "inside the box: {:?}",
            rendered[1]
        );
    }

    #[test]
    fn the_text_sits_one_column_inside_the_box() {
        let rendered = rows_of(&render(
            &editor_with("hello"),
            20,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying::default(),
        ));
        assert_eq!(
            rendered[1], "│ hello            │",
            "one column of padding, bars at both edges"
        );
    }

    #[test]
    fn the_cursor_cell_is_inverted_in_place() {
        let mut editor = editor_with("abc");
        editor.home();
        let lines = render(
            &editor,
            20,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying::default(),
        )
        .lines;
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), "a");
    }

    #[test]
    fn a_cursor_at_the_end_inverts_an_added_space() {
        let lines = render(
            &editor_with("ab"),
            20,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying::default(),
        )
        .lines;
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), " ");
    }

    #[test]
    fn the_rules_span_the_full_width() {
        let lines = render(
            &editor_with("x"),
            30,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying::default(),
        )
        .lines;
        for index in [0, lines.len() - 1] {
            let width: usize = lines[index]
                .spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum();
            assert_eq!(width, 30);
        }
    }

    #[test]
    fn a_long_prompt_scrolls_and_the_rules_say_how_much_is_hidden() {
        let body = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rendered = rows_of(&render(
            &editor_with(&body),
            40,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying::default(),
        ));
        assert!(rendered[0].contains("↑"), "{:?}", rendered[0]);
        assert!(rendered[0].contains("more"), "{:?}", rendered[0]);
        assert_eq!(
            rendered.last().map(String::as_str).map(|s| s.contains('↓')),
            Some(false),
            "the cursor is on the last line, so nothing is hidden below"
        );
    }

    #[test]
    fn scrolling_up_reports_the_lines_below() {
        let body = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = editor_with(&body);
        for _ in 0..19 {
            editor.history_prev();
        }
        editor.set_text(&body);
        let rendered = rows_of(&render(
            &editor,
            40,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying::default(),
        ));
        assert_eq!(rendered.len(), visible_rows(24) + 2, "rules plus text rows");
    }

    #[test]
    fn a_short_terminal_still_shows_five_rows() {
        assert_eq!(
            visible_rows(10),
            usize::from(crate::metric::prompt_min_lines())
        );
    }
}

#[cfg(test)]
mod narrow_tests {
    use super::*;

    fn row(width: u16, hint: &str) -> String {
        render(
            &Editor::new(),
            width,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying {
                text: hint,
                caret: None,
                badge_open: false,
                badge: "",
                mode: crate::vim::Mode::default(),
                block: false,
                marked: None,
            },
        )
        .lines[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_narrow_prompt_shortens_the_hint_rather_than_cutting_it() {
        // A line cut in half looks broken, so a narrow screen falls back to the short hint.
        let line = row(20, "a line far too long for twenty columns");
        assert!(line.chars().count() <= 20, "{line:?}");
        let said = line.trim().trim_matches('│').trim();
        assert!(
            said == crate::glyph::placeholder_short() || said.is_empty(),
            "{said:?} is neither the short hint nor nothing"
        );
    }

    #[test]
    fn a_prompt_with_no_room_at_all_says_nothing() {
        let line = row(6, "anything at all");
        assert!(line.chars().count() <= 6, "{line:?}");
    }

    #[test]
    fn a_wide_prompt_draws_what_it_was_handed() {
        // What the box is saying is the caller's business — see `crate::tease` — and this draws it.
        let line = row(80, "let's scan the project");
        let shown = line.trim().trim_matches('│').trim();
        assert_eq!(shown, "let's scan the project");
    }
}
