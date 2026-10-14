//! Drawing a picker. The rows are [`crate::menu`]'s, which the completion popup also uses.

use super::Picker;
use ratatui::text::Line;

/// Draw the list.
#[must_use]
pub fn render(picker: &Picker, width: u16) -> Vec<Line<'static>> {
    let window = picker.window();
    if picker.choices.is_empty() {
        return vec![crate::menu::heading(
            &picker.title,
            &format!("  nothing matches \u{201c}{}\u{201d}", picker.query()),
            width,
        )];
    }
    let shown = picker.choices.get(window.clone()).unwrap_or_default();
    let value_width = shown
        .iter()
        .map(|c| c.value.chars().count())
        .max()
        .unwrap_or(0);

    // How much is out of view in each direction, not just where you are.
    let above = window.start;
    let below = picker.choices.len().saturating_sub(window.end);
    let mut scroll = String::new();
    if above > 0 {
        scroll.push_str(&format!("  ↑{above}"));
    }
    if below > 0 {
        scroll.push_str(&format!("  ↓{below}"));
    }
    let note = if let Some(said) = &picker.notice {
        format!("  {said}")
    } else if picker.query().is_empty() {
        format!(
            "  {} of {}{scroll}",
            picker.selected + 1,
            picker.choices.len()
        )
    } else {
        // Shown in the heading rather than the prompt: this is not an edit of the prompt.
        format!(
            "  {} of {}  ▸ {}{scroll}",
            picker.selected + 1,
            picker.choices.len(),
            picker.query()
        )
    };

    let mut out = vec![crate::menu::heading(&picker.title, &note, width)];
    // What is being decided about, above the answers a person may give, indented off the heading.
    out.extend(picker.drawn_about().into_iter().map(|row| {
        let mut spans = vec![ratatui::text::Span::raw("  ")];
        spans.extend(row.spans);
        ratatui::text::Line::from(spans)
    }));
    let table = (!picker.columns().is_empty()).then(|| super::table::Table::new(picker, width));
    if let Some(table) = &table {
        out.push(table.header());
    }
    out.extend(shown.iter().enumerate().flat_map(|(offset, choice)| {
        let selected = window.start + offset == picker.selected;
        if let Some(table) = &table {
            return vec![table.choice(choice, selected, picker.query())];
        }
        let row = crate::menu::row(
            &crate::menu::Row {
                value: &choice.value,
                detail: if picker.detail_below {
                    ""
                } else {
                    &choice.detail
                },
                selected,
                ready: choice.ready,
                value_width,
            },
            picker.query(),
            width,
        );
        if picker.detail_below {
            let mut rows = vec![row];
            for detail in choice.detail.split('\n') {
                rows.push(crate::menu::row(
                    &crate::menu::Row {
                        value: "",
                        detail,
                        selected,
                        ready: choice.ready,
                        value_width: 0,
                    },
                    "",
                    width,
                ));
            }
            rows
        } else {
            vec![row]
        }
    }));
    // The question last, under the row it is about, so the row stays under your eye while you
    // decide and the list does not move.
    if let Some(asking) = picker.asking() {
        out.push(asked(asking, width));
    }
    out
}

/// The question and its two answers, on one row.
///
/// The highlighted answer is filled and the other is not: one difference, and it is the one being
/// asked about. Delete again answers yes, so the key that asked is the key that agrees.
fn asked(asking: &super::Confirm, width: u16) -> Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;

    let picked = Style::default()
        .fg(crate::colour::text())
        .add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let plain = Style::default().fg(crate::colour::muted());
    let answer =
        |label: &str, on: bool| Span::styled(format!(" {label} "), if on { picked } else { plain });

    // The answers always fit: a question cut short still reads, a `[ yes ]` cut in half does not.
    let room = usize::from(width).saturating_sub(18);
    let question: String = if asking.question.chars().count() > room {
        asking
            .question
            .chars()
            .take(room.saturating_sub(1))
            .chain(crate::glyph::ellipsis().chars())
            .collect()
    } else {
        asking.question.clone()
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(question, Style::default().fg(crate::colour::warning())),
        Span::raw("  "),
        answer("yes", asking.yes),
        Span::raw(" "),
        answer("no", !asking.yes),
    ])
}
