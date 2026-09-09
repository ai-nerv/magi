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
    let value_width = picker.choices[window.clone()]
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
    // What is being decided about, above the answers a person may give.
    out.extend(picker.asking_about().iter().map(|row| {
        ratatui::text::Line::from(ratatui::text::Span::styled(
            format!("  {row}"),
            ratatui::style::Style::default().fg(crate::colour::muted()),
        ))
    }));
    out.extend(
        picker.choices[window.clone()]
            .iter()
            .enumerate()
            .map(|(offset, choice)| {
                crate::menu::row(
                    &crate::menu::Row {
                        value: &choice.value,
                        detail: &choice.detail,
                        selected: window.start + offset == picker.selected,
                        ready: choice.ready,
                        value_width,
                    },
                    picker.query(),
                    width,
                )
            }),
    );
    out
}
