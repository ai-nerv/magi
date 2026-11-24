//! Drawing a picker. The rows are [`crate::menu`]'s, which the completion popup also uses.

use super::Picker;
use ratatui::text::Line;

/// Draw the list.
#[must_use]
pub fn render(picker: &Picker, width: u16) -> Vec<Line<'static>> {
    let window = picker.window();
    let hint = picker
        .hint_text()
        .map(|hint| format!("  · {hint}"))
        .unwrap_or_default();
    if picker.choices.is_empty() {
        let empty_note = if picker.query().is_empty() {
            format!("  none available{hint}")
        } else {
            format!("  nothing matches \u{201c}{}\u{201d}{hint}", picker.query())
        };
        let mut out = vec![crate::menu::heading(&picker.title, &empty_note, width)];
        if picker.has_tabs() {
            out.push(tabs_line(picker.tab_labels(), picker.active_tab(), width));
        }
        return out;
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
            "  {} of {}{scroll}{hint}",
            picker.selected + 1,
            picker.choices.len()
        )
    } else {
        // Shown in the heading rather than the prompt: this is not an edit of the prompt.
        format!(
            "  {} of {}  ▸ {}{scroll}{hint}",
            picker.selected + 1,
            picker.choices.len(),
            picker.query()
        )
    };

    let mut out = vec![crate::menu::heading(&picker.title, &note, width)];
    if picker.has_tabs() {
        out.push(tabs_line(picker.tab_labels(), picker.active_tab(), width));
    }
    if let Some(caption) = picker.caption() {
        out.push(Line::from(format!("  {caption}")));
    }
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

fn tabs_line(tabs: &[String], active: usize, width: u16) -> Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;

    let room = usize::from(width);
    if room == 0 {
        return Line::default();
    }
    let active_style = Style::default()
        .fg(crate::colour::accent())
        .add_modifier(Modifier::BOLD | Modifier::REVERSED);
    let dim = Style::default().fg(crate::colour::dim());
    let widths: Vec<usize> = tabs.iter().map(|name| name.chars().count() + 2).collect();
    let mut start = 0usize;
    let mut end = tabs.len();
    let span_width = |from: usize, to: usize| -> usize {
        let labels: usize = widths.get(from..to).unwrap_or_default().iter().sum();
        let gaps = to.saturating_sub(from).saturating_sub(1);
        labels + gaps
    };
    let budget = room.saturating_sub(2);
    if span_width(start, end) > budget {
        let mut from = active.min(tabs.len().saturating_sub(1));
        let mut to = (from + 1).min(tabs.len());
        let reserve = |f: usize, t: usize| usize::from(f > 0) * 2 + usize::from(t < tabs.len()) * 2;
        loop {
            let mut grew = false;
            if to < tabs.len() && span_width(from, to + 1) + reserve(from, to + 1) <= budget {
                to += 1;
                grew = true;
            }
            if from > 0 && span_width(from - 1, to) + reserve(from - 1, to) <= budget {
                from -= 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        start = from;
        end = to;
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let push_fit =
        |spans: &mut Vec<Span<'static>>, used: &mut usize, text: String, style: Style| {
            if *used >= room {
                return;
            }
            let left = room - *used;
            let count = text.chars().count();
            if count <= left {
                *used += count;
                spans.push(Span::styled(text, style));
            } else {
                let cut: String = text.chars().take(left).collect();
                *used = room;
                spans.push(Span::styled(cut, style));
            }
        };
    push_fit(&mut spans, &mut used, "  ".into(), Style::default());
    if start > 0 {
        push_fit(&mut spans, &mut used, "‹ ".into(), dim);
    }
    for (offset, name) in tabs.get(start..end).unwrap_or_default().iter().enumerate() {
        let index = start + offset;
        if offset > 0 {
            push_fit(&mut spans, &mut used, " ".into(), Style::default());
        }
        let style = if index == active { active_style } else { dim };
        push_fit(&mut spans, &mut used, format!(" {name} "), style);
    }
    if end < tabs.len() {
        push_fit(&mut spans, &mut used, " ›".into(), dim);
    }
    if used < room {
        spans.push(Span::raw(" ".repeat(room - used)));
    }
    Line::from(spans)
}
