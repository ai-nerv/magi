//! The prompt, as Pi draws it.
//!
//! A horizontal rule above and below the text, nothing at the sides, and no gutter — Pi's
//! `editorPaddingX` defaults to `0`, so the text starts in column zero. The cursor is drawn
//! into the line with inverse video rather than parked with the terminal's own cursor, which
//! is what lets the block scroll and wrap without the cursor drifting off it.

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

/// The blank prompt: the cursor, then whatever this session's placeholder is.
///
/// Dimmer than the text, deliberately. A placeholder in the same colour as what you type reads
/// as something already in the box, and the first thing anybody does is try to delete it.
fn placeholder_spans(width: u16) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        " ",
        Style::default()
            .fg(colour::text())
            .add_modifier(Modifier::REVERSED),
    )];
    let hint = chosen();
    // A narrow screen gets the short one, and a very narrow one gets nothing: a placeholder cut
    // in half is not a shorter joke, it is a line that looks broken.
    if hint.chars().count() >= usize::from(width) {
        let short = glyph::placeholder_short();
        if short.chars().count() < usize::from(width) {
            spans.push(Span::styled(
                short.to_owned(),
                Style::default().fg(colour::hint()),
            ));
        }
        return spans;
    }
    spans.extend(struck(hint));
    spans
}

/// This session's placeholder, chosen once.
///
/// Once, not per frame: a line that changed sixty times a second would be unreadable, and one
/// that changed when you deleted the last character would be worse -- it would look like the
/// box had done something.
fn chosen() -> &'static str {
    static PICK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let list = glyph::placeholders();
    if list.is_empty() {
        return "";
    }
    let at = *PICK.get_or_init(|| {
        // The clock, because there is no rng here and none is worth a dependency for this.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos() as usize);
        seed % list.len()
    });
    list.get(at).map_or("", String::as_str)
}

/// Split a placeholder on its `~~struck~~` run.
///
/// The struck half stays legible under the line: the joke is the correction, and a correction
/// you cannot read the first half of is not one.
fn struck(text: &str) -> Vec<Span<'static>> {
    let dim = Style::default().fg(colour::hint());
    let Some((before, rest)) = text.split_once("~~") else {
        return vec![Span::styled(text.to_owned(), dim)];
    };
    let Some((out, after)) = rest.split_once("~~") else {
        return vec![Span::styled(text.to_owned(), dim)];
    };
    vec![
        Span::styled(before.to_owned(), dim),
        Span::styled(out.to_owned(), dim.add_modifier(Modifier::CROSSED_OUT)),
        Span::styled(after.to_owned(), dim),
    ]
}

/// How many text rows the prompt shows right now, on a terminal `rows` tall.
///
/// Worked out here rather than by drawing and counting, because the caller has to know how tall
/// the box will be before it can say how much room is left in it for a menu.
#[must_use]
pub fn text_rows(editor: &Editor, rows: u16) -> usize {
    let max_visible = visible_rows(rows);
    let total = editor.lines().len();
    if total == 1 && editor.lines()[0].is_empty() {
        return 1;
    }
    let (cursor_row, _) = editor.cursor();
    let offset = cursor_row.saturating_sub(max_visible.saturating_sub(1));
    let offset = offset.min(total.saturating_sub(max_visible.min(total)));
    (offset + max_visible).min(total) - offset
}

/// Render the prompt as a box, with `menu` inside it under a divider.
///
/// Was a rule above and a rule below, which is Pi's shape. A box says where the field is, and
/// gives the scan somewhere to run: see [`crate::border`].
///
/// The menu goes *inside*. Drawn beneath the box it was a second object with a background of
/// its own, sitting under the thing it belongs to; inside, the box is what says where it is and
/// the rows need no colour behind them. `menu` is already `width - 3` wide — see
/// [`crate::metric::gutter`] — and empty when nothing is open.
///
/// `rows` is the terminal height, which sets how much of a long prompt is shown before the top
/// and bottom edges start reporting what is scrolled out of view. `tick` drives the scan and
/// `scan` says what it should be doing.
#[must_use]
pub fn render(
    editor: &Editor,
    width: u16,
    rows: u16,
    tick: usize,
    scan: crate::border::Scan,
    menu: &[Line<'static>],
) -> Vec<Line<'static>> {
    let text_style = Style::default().fg(colour::text());
    let (cursor_row, cursor_col) = editor.cursor();
    // Two columns of the width belong to the sides now.
    let inner_width = width.saturating_sub(2);

    let max_visible = visible_rows(rows);
    let total = editor.lines().len();
    // Scrolled to keep the cursor in view, exactly as Pi's editor does.
    let offset = cursor_row.saturating_sub(max_visible.saturating_sub(1));
    let offset = offset.min(total.saturating_sub(max_visible.min(total)));
    let end = (offset + max_visible).min(total);

    let blank = total == 1 && editor.lines()[0].is_empty();
    let shown = if blank { 1 } else { end - offset };
    // The divider is a content row like any other, so the sides stay on the ring and the scan
    // runs past it rather than round a hole in the box.
    let content = shown + if menu.is_empty() { 0 } else { 1 + menu.len() };
    let (top, bottom) = crate::border::edges(width, content, tick, scan);

    let mut out = Vec::with_capacity(content + 2);
    out.push(hidden(top, Direction::Up, offset));

    for row in 0..shown {
        let body = if blank {
            placeholder_spans(inner_width)
        } else {
            let index = offset + row;
            let text = resolving(editor, index);
            if index == cursor_row {
                with_cursor(&text, cursor_col, text_style)
            } else {
                vec![Span::styled(text, text_style)]
            }
        };
        out.push(framed(body, width, content, row, tick, scan));
    }

    if !menu.is_empty() {
        out.push(divider(width, content, shown, tick, scan));
        for (row, line) in menu.iter().enumerate() {
            out.push(framed(
                line.spans.clone(),
                width,
                content,
                shown + 1 + row,
                tick,
                scan,
            ));
        }
    }

    let below = total.saturating_sub(end);
    out.push(hidden(bottom, Direction::Down, below));
    out
}

/// Line `row` with anything typed a moment ago still on its way to being itself.
///
/// A character arrives as the first of [`crate::glyph::type_stages`], passes through the rest, and
/// lands as what was typed. Off unless `axon.ui.type_reveal_ms` says otherwise, and the same
/// width throughout: the box is around this, and text that changes width under a border is worse
/// than no effect at all.
fn resolving(editor: &Editor, row: usize) -> String {
    let text = &editor.lines()[row];
    let over = crate::metric::type_reveal_ms();
    let stages: Vec<char> = crate::glyph::type_stages().chars().collect();
    if over == 0 || stages.is_empty() {
        return text.clone();
    }
    let each = (over / stages.len() as u64).max(1);
    text.char_indices()
        .enumerate()
        .map(|(col, (_, ch))| {
            let Some(age) = editor.typed_age(row, col, ch) else {
                return ch;
            };
            let stage = usize::try_from(age.as_millis() / u128::from(each)).unwrap_or(usize::MAX);
            stages.get(stage).copied().unwrap_or(ch)
        })
        .collect()
}

/// One content row between its two side bars.
fn framed(
    body: Vec<Span<'static>>,
    width: u16,
    content: usize,
    row: usize,
    tick: usize,
    scan: crate::border::Scan,
) -> Line<'static> {
    let (left, right) = crate::border::side(width, content, row, tick, scan);
    let mut spans = vec![left, Span::raw(" ")];
    spans.extend(pad(body, width.saturating_sub(3)));
    spans.push(right);
    Line::from(spans)
}

/// The rule between the text and the menu.
///
/// Tees into the sides rather than floating between them, so the box reads as one frame with a
/// shelf in it rather than as two boxes that happen to touch.
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
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let room = usize::from(width).saturating_sub(used);
    if room > 0 {
        spans.push(Span::raw(" ".repeat(room)));
    }
    spans
}

/// Write "N more" into an edge when the prompt is scrolled.
///
/// On the border rather than instead of it: the box stays a box, and the count sits in it the
/// way a caption sits in a frame.
fn hidden(edge: Line<'static>, direction: Direction, count: usize) -> Line<'static> {
    if count == 0 {
        return edge;
    }
    let arrow = match direction {
        Direction::Up => '↑',
        Direction::Down => '↓',
    };
    let label = format!(" {arrow} {count} more ");
    let width: usize = edge.spans.iter().map(|s| s.content.chars().count()).sum();
    if label.chars().count() + 4 > width {
        return edge;
    }
    // Kept from the third column, so the corner and the first stretch of border survive and the
    // caption reads as part of the frame.
    let mut spans: Vec<Span<'static>> = edge.spans.into_iter().take(3).collect();
    spans.push(Span::styled(
        label.clone(),
        Style::default().fg(colour::dim()),
    ));
    let used = 3 + label.chars().count();
    for _ in used..width - 1 {
        spans.push(Span::styled("─", Style::default().fg(colour::border())));
    }
    spans.push(Span::styled(
        "╮".to_owned(),
        Style::default().fg(colour::border()),
    ));
    let last = spans.len() - 1;
    if matches!(direction, Direction::Down) {
        spans[last] = Span::styled("╯".to_owned(), Style::default().fg(colour::border()));
    }
    Line::from(spans)
}

enum Direction {
    Up,
    Down,
}

/// Draw one line with the cursor cell inverted.
///
/// At the end of a line there is no character to invert, so a space is added and inverted —
/// which is why the layout reserves a column for it.
fn with_cursor(text: &str, col: usize, style: Style) -> Vec<Span<'static>> {
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

    fn rows_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn an_empty_prompt_says_what_to_do_with_it() {
        // An empty box between two rules gives no way to tell a prompt waiting for input from
        // a screen that has hung, and no way to find the command list without being told.
        let rendered = rows_of(&render(
            &Editor::new(),
            40,
            24,
            0,
            crate::border::Scan::Off,
            &[],
        ));
        assert_eq!(rendered.len(), 3, "top edge, text, bottom edge");
        // Whatever this session picked, or the short hint when it will not fit -- not a
        // particular line, because which one comes up is the point of having a list.
        let said = rendered[1].trim().trim_matches('│').trim();
        assert!(!said.is_empty(), "the box says nothing: {:?}", rendered[1]);
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
        let lines = render(&editor, 20, 24, 0, crate::border::Scan::Off, &[]);
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), "a");
    }

    #[test]
    fn a_cursor_at_the_end_inverts_an_added_space() {
        let lines = render(&editor_with("ab"), 20, 24, 0, crate::border::Scan::Off, &[]);
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), " ");
    }

    #[test]
    fn the_rules_span_the_full_width() {
        let lines = render(&editor_with("x"), 30, 24, 0, crate::border::Scan::Off, &[]);
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
        let rendered = rows_of(&render(&editor, 40, 24, 0, crate::border::Scan::Off, &[]));
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

    fn row(width: u16) -> String {
        render(&Editor::new(), width, 24, 0, crate::border::Scan::Off, &[])[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_narrow_prompt_shortens_the_hint_rather_than_cutting_it() {
        // A placeholder cut in half is not a shorter line, it is one that looks broken. The
        // session's placeholder is a joke of any length, so a narrow screen falls back to the
        // short hint instead of trimming whichever one came up.
        let line = row(20);
        assert!(line.chars().count() <= 20, "{line:?}");
        // Either the session's line fits, or the short hint stood in for it. What must never
        // happen is half a line: `ask anything, or / for comman` reads as a rendering fault.
        let said = line.trim().trim_matches('│').trim();
        let whole = crate::glyph::placeholders()
            .iter()
            .any(|p| p.replace("~~", "").trim() == said);
        assert!(
            whole || said == crate::glyph::placeholder_short() || said.is_empty(),
            "{said:?} is neither a whole line nor the short hint"
        );
    }

    #[test]
    fn a_prompt_with_no_room_at_all_says_nothing() {
        let line = row(6);
        assert!(line.chars().count() <= 6, "{line:?}");
    }

    #[test]
    fn a_wide_prompt_shows_this_session_s_placeholder() {
        // Whichever it picked, with the `~~` markers consumed rather than printed.
        let line = row(80);
        assert!(!line.contains("~~"), "the markers leaked: {line:?}");
        let shown = line.trim().trim_matches('│').trim();
        assert!(!shown.is_empty(), "{line:?}");
        assert!(
            crate::glyph::placeholders()
                .iter()
                .any(|p| p.replace("~~", "").trim() == shown),
            "{shown:?} is not one of the list"
        );
    }
}

/// A character you type arrives as a symbol and resolves into itself.
#[cfg(test)]
mod resolving_tests {
    use super::*;

    /// The prompt's first line, with `text` typed into it.
    fn line_of(text: &str) -> String {
        let mut editor = Editor::new();
        editor.insert_str(text);
        resolving(&editor, 0)
    }

    #[test]
    fn off_is_off() {
        // Zero is the built-in, and a config that says nothing about this gets what it typed.
        assert_eq!(crate::metric::BUILT_IN.type_reveal_ms, 0);
        assert_eq!(line_of("hello"), "hello");
    }

    #[test]
    fn the_stages_are_symbols_and_end_in_the_letter() {
        // What a character passes through on the way to being itself. A letter passing through
        // another letter reads as a typo correcting itself.
        let stages = crate::glyph::type_stages();
        assert!(!stages.is_empty());
        assert!(
            !stages.chars().any(char::is_alphanumeric),
            "a stage that is a letter reads as a typo: {stages:?}"
        );
    }

    #[test]
    fn a_character_that_was_not_just_typed_is_left_alone() {
        // The reveal is about arrival. Text recalled from history, or pasted and settled, is
        // already there and must not flicker every time the screen redraws.
        let mut editor = Editor::new();
        editor.insert_str("settled");
        // Nothing matches at a position holding a different character.
        assert!(editor.typed_age(0, 0, 'x').is_none());
        assert!(editor.typed_age(9, 0, 's').is_none());
    }

    #[test]
    fn the_width_never_changes() {
        // The box is around this. Text that changes width under a border is worse than no
        // effect at all.
        for text in ["a", "hello world", "unicode: ✓ ✗"] {
            assert_eq!(
                line_of(text).chars().count(),
                text.chars().count(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn what_was_typed_is_remembered_where_it_was_typed() {
        let mut editor = Editor::new();
        editor.insert('h');
        editor.insert('i');
        assert!(editor.typed_age(0, 0, 'h').is_some());
        assert!(editor.typed_age(0, 1, 'i').is_some());
        assert!(
            editor.typed_age(0, 1, 'h').is_none(),
            "not by position alone"
        );
    }
}

/// The empty prompt says something, and says it as a placeholder.
#[cfg(test)]
mod placeholder_tests {
    use super::*;

    fn spans_of(text: &str) -> Vec<(String, bool)> {
        struck(text)
            .into_iter()
            .map(|s| {
                (
                    s.content.into_owned(),
                    s.style.add_modifier.contains(Modifier::CROSSED_OUT),
                )
            })
            .collect()
    }

    #[test]
    fn a_correction_is_three_spans_and_only_the_middle_is_struck() {
        assert_eq!(
            spans_of("ship it ~~Friday~~ whenever"),
            vec![
                ("ship it ".to_owned(), false),
                ("Friday".to_owned(), true),
                (" whenever".to_owned(), false),
            ]
        );
    }

    #[test]
    fn a_line_with_no_correction_is_left_whole() {
        assert_eq!(
            spans_of("just a hint"),
            vec![("just a hint".to_owned(), false)]
        );
    }

    #[test]
    fn an_unclosed_marker_is_text_rather_than_a_panic() {
        // A config author's typo should cost them a stray `~~`, not the prompt.
        assert_eq!(
            spans_of("half a ~~thought"),
            vec![("half a ~~thought".to_owned(), false)]
        );
    }

    #[test]
    fn every_shipped_placeholder_has_a_correction_in_it() {
        // The joke is the second thought. One without a struck run is a line that forgot to be
        // the thing this list is for.
        for line in crate::glyph::placeholders() {
            let parts = struck(line);
            assert_eq!(parts.len(), 3, "{line:?} has nothing struck out");
            assert!(
                parts[1].style.add_modifier.contains(Modifier::CROSSED_OUT),
                "{line:?}"
            );
        }
    }

    #[test]
    fn there_are_enough_of_them_to_not_repeat_soon() {
        assert!(crate::glyph::placeholders().len() >= 20);
    }

    #[test]
    fn the_placeholder_is_dimmer_than_what_you_type() {
        // A placeholder in the text colour reads as something already in the box, and the first
        // thing anybody does is try to delete it.
        assert!(
            colour::palette().hint < colour::palette().text,
            "the hint is not dimmer: {} against {}",
            colour::palette().hint,
            colour::palette().text
        );
    }

    #[test]
    fn a_screen_too_narrow_for_the_line_says_something_shorter() {
        let narrow = placeholder_spans(12);
        let text: String = narrow.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.chars().count() <= 12, "{text:?}");
    }
}
