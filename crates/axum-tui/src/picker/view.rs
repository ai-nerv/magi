//! Drawing a picker.
//!
//! Split from the state machine under THE RULE: both halves grew — the machine gained readiness
//! ranking, the view gained a highlight bar and scroll counters — and the file holding both
//! crossed 800 lines while each half was still the right size.

use super::Picker;
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Fill a row out to the full width, so a highlight bar reaches the edge.
///
/// Without this the selected row's background stops wherever its text stops, which reads as a
/// ragged block rather than a bar and is worse than no highlight at all.
fn pad(
    mut spans: Vec<Span<'static>>,
    width: u16,
    bg: Option<ratatui::style::Color>,
) -> Vec<Span<'static>> {
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let room = usize::from(width).saturating_sub(used);
    if room > 0 {
        let style = bg.map_or_else(Style::default, |bg| Style::default().bg(bg));
        spans.push(Span::styled(" ".repeat(room), style));
    }
    spans
}

/// Draw the list.
#[must_use]
pub fn render(picker: &Picker, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let window = picker.window();
    if picker.choices.is_empty() {
        return vec![Line::from(crate::fit(
            vec![
                Span::styled(
                    picker.title.clone(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  nothing matches \u{201c}{}\u{201d}", picker.query()),
                    Style::default().fg(theme.warning),
                ),
            ],
            usize::from(width),
        ))];
    }
    let value_width = picker.choices[window.clone()]
        .iter()
        .map(|c| c.value.chars().count())
        .max()
        .unwrap_or(0);

    // A heading, because unlike the completion popup this is not obviously about what you just
    // typed: it appears because you asked a question, and it should say which one.
    //
    // The counter says how much is out of view in each direction rather than just where you
    // are. A list of fifty-three in a window of eight gave no sign there was anything above or
    // below the eight, so moving through it felt like the list was changing under you.
    let above = window.start;
    let below = picker.choices.len().saturating_sub(window.end);
    let mut scroll = String::new();
    if above > 0 {
        scroll.push_str(&format!("  ↑{above}"));
    }
    if below > 0 {
        scroll.push_str(&format!("  ↓{below}"));
    }
    let position = if let Some(said) = &picker.notice {
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
    let mut out = vec![Line::from(crate::fit(
        vec![
            Span::styled(
                picker.title.clone(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                position,
                Style::default().fg(if picker.notice.is_some() {
                    theme.warning
                } else {
                    theme.dim
                }),
            ),
        ],
        usize::from(width),
    ))];

    out.extend(
        picker.choices[window.clone()]
            .iter()
            .enumerate()
            .map(|(offset, choice)| {
                let selected = window.start + offset == picker.selected;
                // A filled bar across the whole width, not an arrow in the margin. A menu you
                // move through should show where you are the way a menu does; the arrow alone
                // left the eye hunting for two characters in a column of near-identical rows.
                let row_bg = if selected {
                    Some(theme.user_message_bg)
                } else {
                    None
                };
                let base = |style: Style| match row_bg {
                    Some(bg) => style.bg(bg),
                    None => style,
                };

                let (marker, value_style) = if selected {
                    ("❯ ", Style::default().fg(theme.accent))
                } else if choice.ready {
                    ("  ", Style::default().fg(theme.text))
                } else {
                    // Dimmed rather than hidden: it is real, it is just not ready.
                    ("  ", Style::default().fg(theme.dim))
                };

                let gap = value_width - choice.value.chars().count() + 2;
                let detail_style = if choice.ready {
                    Style::default().fg(theme.muted)
                } else {
                    Style::default().fg(theme.warning)
                };
                let spans = vec![
                    Span::styled(marker, base(Style::default().fg(theme.accent))),
                    Span::styled(
                        choice.value.clone(),
                        base(if selected {
                            value_style.add_modifier(Modifier::BOLD)
                        } else {
                            value_style
                        }),
                    ),
                    Span::styled(
                        format!("{}{}", " ".repeat(gap), choice.detail),
                        base(detail_style),
                    ),
                ];
                // Padded to the full width so the bar reaches the edge of the terminal rather
                // than stopping wherever the text happened to end.
                Line::from(pad(crate::fit(spans, usize::from(width)), width, row_bg))
            }),
    );
    out
}
