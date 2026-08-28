//! Drawing a picker.
//!
//! Split from the state machine under THE RULE. The rows themselves are [`crate::menu`]'s, which
//! the completion popup also uses: they are the same object seen twice and had drifted into two
//! different-looking lists.

use super::Picker;
use crate::theme::Theme;
use ratatui::text::Line;

/// Draw the list.
#[must_use]
pub fn render(picker: &Picker, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let window = picker.window();
    if picker.choices.is_empty() {
        return vec![crate::menu::heading(
            &picker.title,
            &format!("  nothing matches \u{201c}{}\u{201d}", picker.query()),
            width,
            theme,
        )];
    }
    let value_width = picker.choices[window.clone()]
        .iter()
        .map(|c| c.value.chars().count())
        .max()
        .unwrap_or(0);

    // How much is out of view in each direction, not just where you are. A list of fifty-three
    // in a window of eight gave no sign there was anything above or below the eight, so moving
    // through it felt like the list was changing under you.
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
        // The query is shown in the heading rather than in the prompt, because the prompt is
        // holding whatever it was holding and this is not an edit of it.
        format!(
            "  {} of {}  ▸ {}{scroll}",
            picker.selected + 1,
            picker.choices.len(),
            picker.query()
        )
    };

    let mut out = vec![crate::menu::heading(&picker.title, &note, width, theme)];
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
                    theme,
                )
            }),
    );
    out
}
