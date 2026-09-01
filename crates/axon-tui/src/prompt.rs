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
pub(crate) fn placeholder_spans(
    width: u16,
    hint: &str,
    caret: Option<usize>,
) -> Vec<Span<'static>> {
    // A narrow screen gets the short one, and a very narrow one gets nothing: a placeholder cut
    // in half is not a shorter line, it is one that looks broken.
    let hint = if hint.chars().count() < usize::from(width) {
        hint
    } else if glyph::placeholder_short().chars().count() < usize::from(width) {
        glyph::placeholder_short()
    } else {
        ""
    };

    let dim = Style::default().fg(colour::hint());
    // The real cursor, on the first letter rather than in front of it. It is where typing would
    // land, and typing lands on column zero whatever the box happens to be saying.
    let block = Style::default()
        .fg(colour::text())
        .add_modifier(Modifier::REVERSED);
    // The second one, where the box is editing itself. Dimmer, because it is not yours: two
    // cursors of equal weight is a screen with two places to type.
    let writing = dim.add_modifier(Modifier::REVERSED);

    let mut spans = Vec::new();
    let letters: Vec<char> = hint.chars().collect();
    if letters.is_empty() {
        return vec![Span::styled(" ", block)];
    }
    for (at, letter) in letters.iter().enumerate() {
        let style = if at == 0 {
            block
        } else if caret == Some(at) {
            writing
        } else {
            dim
        };
        spans.push(Span::styled(letter.to_string(), style));
    }
    // Past the last letter, which is where a caret sits while text is being added to the end.
    if caret == Some(letters.len()) {
        spans.push(Span::styled(" ", writing));
    }
    spans
}

/// How many text rows the prompt shows right now, on a terminal `rows` tall.
///
/// Worked out here rather than by drawing and counting, because the caller has to know how tall
/// the box will be before it can say how much room is left in it for a menu.
#[must_use]
pub fn text_rows(editor: &Editor, rows: u16, width: u16, badge: &str) -> usize {
    if editor.lines().len() == 1 && editor.lines()[0].is_empty() {
        return 1;
    }
    // Folded rows, not logical lines. A caller sizing the box from logical lines gives a
    // three-line prompt one row and then draws three into it.
    let (visual, caret, _) = crate::fold::fold_all(editor, crate::fold::text_room(width, badge));
    let total = visual.len().max(1);
    let max_visible = visible_rows(rows);
    let offset = caret.saturating_sub(max_visible.saturating_sub(1));
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
/// `scan` says what it should be doing. `placeholder` is which line the empty box shows, which
/// the caller remembers because it changes when the prompt empties rather than per frame.
#[must_use]
pub fn render(
    editor: &Editor,
    width: u16,
    rows: u16,
    tick: usize,
    scan: crate::border::Scan,
    menu: &[Line<'static>],
    saying: crate::tease::Saying<'_>,
) -> Vec<Line<'static>> {
    let badge = saying.badge;
    let text_style = Style::default().fg(colour::text());
    // What is left for text once the sides, the padding and the badge's strip are taken out.
    let room = crate::fold::text_room(width, badge);
    let blank = editor.lines().len() == 1 && editor.lines()[0].is_empty();

    // Folded first, then scrolled over the *folded* rows. Scrolling over logical lines and
    // drawing folded ones disagree about how far down the cursor is, which puts it off screen
    // exactly when a line is long enough to need wrapping.
    let (visual, caret_row, caret_col) = crate::fold::fold_all(editor, room);
    let total_rows = visual.len().max(1);
    let max_visible = visible_rows(rows);
    let offset = caret_row.saturating_sub(max_visible.saturating_sub(1));
    let offset = offset.min(total_rows.saturating_sub(max_visible.min(total_rows)));
    let end = (offset + max_visible).min(total_rows);
    let shown = if blank { 1 } else { end - offset };
    // The divider is a content row like any other, so the sides stay on the ring and the scan
    // runs past it rather than round a hole in the box.
    let content = shown + if menu.is_empty() { 0 } else { 1 + menu.len() };
    let (top, bottom) = crate::border::edges(width, content, tick, scan);

    let mut out = Vec::with_capacity(content + 2);
    out.push(tagged(hidden(top, Direction::Up, offset), saying.mode));

    for row in 0..shown {
        let body = if blank {
            placeholder_spans(
                u16::try_from(room).unwrap_or(u16::MAX),
                saying.text,
                saying.caret,
            )
        } else {
            let index = offset + row;
            let text = visual.get(index).cloned().unwrap_or_default();
            if index == caret_row {
                with_cursor(&text, caret_col, text_style)
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
            &crate::fold::strip(badge, shown, row),
        ));
    }

    if !menu.is_empty() {
        out.push(divider(width, content, shown, tick, scan));
        for (row, line) in menu.iter().enumerate() {
            let at = shown + 1 + row;
            out.push(framed(
                line.spans.clone(),
                width,
                content,
                at,
                tick,
                scan,
                // No strip: the badge belongs to the box you type in, and a list opened under
                // it is not that. Centred over the text rows alone for the same reason.
                &[],
            ));
        }
    }

    let below = total_rows.saturating_sub(end);
    out.push(hidden(bottom, Direction::Down, below));
    out
}

/// Line `row` with anything typed a moment ago still on its way to being itself.
///
/// A character arrives as the first of [`crate::glyph::type_stages`], passes through the rest, and
/// lands as what was typed. Off unless `axon.ui.type_reveal_ms` says otherwise, and the same
/// width throughout: the box is around this, and text that changes width under a border is worse
/// than no effect at all.
pub(crate) fn resolving(editor: &Editor, row: usize) -> String {
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
    // A folded row keeps the space it broke on, so it can stand one column past the width it was
    // folded at. Invisible on its own, but it shoves whatever follows — the badge — off the end.
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
    let width = columns_of(&edge);
    if label.chars().count() + 8 > width {
        return edge;
    }
    // Right on the top edge, left on the bottom. The mode has the top-left corner and two
    // captions in one place is one caption with something written over it.
    let at = match direction {
        Direction::Up => width - 2 - label.chars().count(),
        Direction::Down => 2,
    };
    caption(edge, &label, Style::default().fg(colour::dim()), at)
}

/// Write the mode onto the top edge of the box.
///
/// On the border, the way the scroll count is, and on the top-left because that is where the
/// eye starts. It is not optional: the prompt opens in normal mode and refuses text until told
/// otherwise, and a modal editor that does not say which mode it is in is a broken keyboard.
///
/// Three letters, always, so the frame does not move when the mode does.
fn tagged(edge: Line<'static>, mode: crate::vim::Mode) -> Line<'static> {
    let label = format!(" {} ", mode.tag());
    if label.chars().count() + 4 > columns_of(&edge) {
        return edge;
    }
    // Insert mode is the one worth noticing, because it is the one where a keystroke changes
    // something. Normal mode sits at the same level as the border it is written on.
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

/// Write `label` over the edge, starting at column `at`.
///
/// Over the columns it covers rather than spliced between spans. An edge is a handful of spans
/// whose boundaries move as the scan travels along it, so cutting at a span index cuts somewhere
/// different every frame — which is how the first version of this ate a corner, and how the
/// version before that rebuilt one by hand and got it wrong on the other edge.
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
        // a screen that has hung. What it says is the caller's -- see `crate::tease` -- and it
        // is drawn where the cursor is not.
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
                badge: "",
                mode: crate::vim::Mode::default(),
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
        );
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
        );
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
        );
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
                badge: "",
                mode: crate::vim::Mode::default(),
            },
        )[1]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
    }

    #[test]
    fn a_narrow_prompt_shortens_the_hint_rather_than_cutting_it() {
        // A line cut in half is not a shorter line, it is one that looks broken. What the box is
        // writing can be any length, so a narrow screen falls back to the short hint.
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
        // The renderer chooses nothing now. What the box is saying is the caller's business --
        // see `crate::tease` -- and this draws it.
        let line = row(80, "let's scan the project");
        let shown = line.trim().trim_matches('│').trim();
        assert_eq!(shown, "let's scan the project");
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
        // Half a line reads as a rendering fault. The short hint stands in instead.
        let narrow = placeholder_spans(12, "a line far too long for twelve columns", None);
        let text: String = narrow.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.chars().count() <= 12, "{text:?}");
    }

    #[test]
    fn a_line_that_fits_is_drawn_whole() {
        let spans = placeholder_spans(40, "let's build something", None);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("let's build something"), "{text:?}");
    }

    #[test]
    fn nothing_is_struck_through_any_more() {
        // The correction is performed by `crate::tease` -- written, then taken back -- rather
        // than drawn with both halves on screen at once.
        let spans = placeholder_spans(60, "the scaffolding is temporary", None);
        assert!(
            spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
            "something is still drawn struck"
        );
    }
}
