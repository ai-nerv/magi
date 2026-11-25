//! Structured decision prompt rendering for tool permissions and interactive questions.

use super::Picker;
use crate::{colour, glyph, wrap};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::BTreeMap;

/// Semantic emphasis for a decision option.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Normal,
    Broad,
    Danger,
}

/// What a tool is asking to do, wrapped dynamically to the prompt box width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Command(String),
    Path(String),
    Host(String),
    Lines(Vec<String>),
}

/// Optional verdict from a second safety model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    pub safe: bool,
    pub rule: String,
    pub reason: String,
}

/// Presentation settings for a fixed-choice decision prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    pub label: String,
    pub subject: Option<Subject>,
    pub advice: Option<Advice>,
    pub queue: Option<String>,
    pub quick_keys: bool,
    pub escape_hint: Option<String>,
    pub tones: BTreeMap<String, Tone>,
}

impl Decision {
    #[must_use]
    pub fn is_warned(&self) -> bool {
        self.advice.as_ref().is_some_and(|advice| !advice.safe)
    }

    #[must_use]
    pub fn tone_of(&self, value: &str) -> Tone {
        self.tones.get(value).copied().unwrap_or(Tone::Normal)
    }
}

/// Draw a structured decision prompt within `width` and optional vertical `max_rows`.
#[must_use]
pub(super) fn render(
    picker: &Picker,
    decision: &Decision,
    width: u16,
    max_rows: Option<usize>,
) -> Vec<Line<'static>> {
    let room = usize::from(width);
    if room == 0 {
        return Vec::new();
    }
    let warned = decision.is_warned();
    let header = heading(picker, decision, width, warned);
    let advice_rows = decision
        .advice
        .as_ref()
        .map(|advice| render_advice(advice, width))
        .unwrap_or_default();
    let choice_rows = render_choices(picker, decision, width);

    let has_subject = decision.subject.is_some();
    let has_advice = !advice_rows.is_empty();
    let fixed_core = 1 + advice_rows.len() + choice_rows.len();
    let total_budget = max_rows.unwrap_or(usize::MAX);

    let mut out = vec![header];
    let want_top_gap = has_advice || has_subject;
    let want_mid_gap = has_advice && has_subject;
    let want_bottom_gap = has_advice || has_subject;
    let mut spare = total_budget.saturating_sub(fixed_core + usize::from(has_subject));

    let top_gap = want_top_gap && spare > 0;
    if top_gap {
        spare -= 1;
    }
    let bottom_gap = want_bottom_gap && spare > 0;
    if bottom_gap {
        spare -= 1;
    }
    let mid_gap = want_mid_gap && spare > 0;
    if mid_gap {
        spare -= 1;
    }

    if top_gap {
        out.push(blank(width));
    }
    if has_advice {
        out.extend(advice_rows);
    }
    if mid_gap {
        out.push(blank(width));
    }
    if let Some(subject) = &decision.subject {
        let subject_budget = (1 + spare).min(8);
        out.extend(render_subject(subject, width, warned, subject_budget));
    }
    if bottom_gap {
        out.push(blank(width));
    }
    out.extend(choice_rows);
    out
}

fn heading(picker: &Picker, decision: &Decision, width: u16, warned: bool) -> Line<'static> {
    let label_color = if warned {
        colour::error()
    } else {
        colour::warning()
    };
    let mut left = vec![Span::styled(
        format!(" {} ", decision.label),
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD),
    )];
    if !picker.title.is_empty() {
        left.push(Span::raw(" "));
        left.push(Span::styled(
            picker.title.clone(),
            Style::default()
                .fg(colour::text())
                .add_modifier(Modifier::BOLD),
        ));
    }
    if warned {
        left.push(Span::raw("  "));
        left.push(Span::styled(
            "! advised against",
            Style::default()
                .fg(colour::error())
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(queue) = &decision.queue {
        left.push(Span::raw("  "));
        left.push(Span::styled(
            format!("({queue})"),
            Style::default()
                .fg(colour::warning())
                .add_modifier(Modifier::BOLD),
        ));
    }

    let hint = picker
        .hint_text()
        .map(ToOwned::to_owned)
        .or_else(|| default_hint(picker, decision, width, &left));

    let room = usize::from(width);
    let left_width: usize = left.iter().map(Span::width).sum();
    if let Some(hint) = hint {
        let hint_width = wrap::columns(&hint) + 1;
        if left_width + 2 + hint_width <= room {
            let gap = room - left_width - hint_width;
            left.push(Span::raw(" ".repeat(gap)));
            left.push(Span::styled(
                format!("{hint} "),
                Style::default().fg(colour::dim()),
            ));
        }
    }
    Line::from(fill(clip(left, room), width, Color::Reset))
}

fn default_hint(
    picker: &Picker,
    decision: &Decision,
    width: u16,
    left: &[Span<'static>],
) -> Option<String> {
    let count = picker.choices.len();
    if count == 0 {
        return None;
    }
    let esc = decision.escape_hint.as_deref().unwrap_or("Esc");
    let keys = if decision.quick_keys && count <= 9 {
        format!("1\u{2013}{count}")
    } else {
        "\u{2191}\u{2193}".to_owned()
    };
    let candidates = [
        format!("{keys} \u{b7} \u{2191}\u{2193} \u{b7} Enter \u{b7} {esc}"),
        format!("{keys} \u{b7} Enter \u{b7} {esc}"),
        format!("{keys} \u{b7} {esc}"),
    ];
    let left_width: usize = left.iter().map(Span::width).sum();
    let room = usize::from(width);
    candidates
        .into_iter()
        .find(|hint| left_width + 3 + wrap::columns(hint) <= room)
}

fn render_advice(advice: &Advice, width: u16) -> Vec<Line<'static>> {
    let room = usize::from(width);
    let (mark, title, ink) = if advice.safe {
        ("\u{2713}", "looks safe", colour::success())
    } else {
        ("!", "advises against", colour::error())
    };
    let mut header = vec![
        Span::raw("  "),
        Span::styled(
            format!("{mark} {title}"),
            Style::default().fg(ink).add_modifier(Modifier::BOLD),
        ),
    ];
    if !advice.rule.is_empty() {
        header.push(Span::styled(
            format!("  \u{b7} {}", advice.rule),
            Style::default().fg(colour::muted()),
        ));
    }
    let mut out = vec![Line::from(fill(clip(header, room), width, Color::Reset))];
    if !advice.reason.is_empty() {
        let inner = room.saturating_sub(6).max(8);
        let reason_style = Style::default().fg(colour::muted());
        for line in wrap::hard(&advice.reason, inner).into_iter().take(3) {
            let spans = vec![Span::raw("    "), Span::styled(line, reason_style)];
            out.push(Line::from(fill(clip(spans, room), width, Color::Reset)));
        }
    }
    out
}

fn render_subject(
    subject: &Subject,
    width: u16,
    warned: bool,
    max_lines: usize,
) -> Vec<Line<'static>> {
    let room = usize::from(width);
    let inner = room.saturating_sub(6).max(4);
    let bg = colour::tool_bg();
    let bar_fg = if warned {
        colour::error()
    } else {
        colour::accent()
    };

    let mut content_lines: Vec<Vec<Span<'static>>> = match subject {
        Subject::Command(command) => {
            let wrapped: Vec<String> = command
                .split('\n')
                .flat_map(|segment| wrap::hard(segment, inner))
                .collect();
            crate::syntax::block("bash", &wrapped, Style::default())
        }
        Subject::Path(path) => wrap::hard(path, inner)
            .into_iter()
            .map(|line| vec![Span::styled(line, Style::default().fg(colour::code_path()))])
            .collect(),
        Subject::Host(host) => wrap::hard(host, inner)
            .into_iter()
            .map(|line| vec![Span::styled(line, Style::default().fg(colour::code_flag()))])
            .collect(),
        Subject::Lines(lines) => lines
            .iter()
            .flat_map(|line| wrap::hard(line, inner))
            .map(|line| vec![Span::styled(line, Style::default().fg(colour::text()))])
            .collect(),
    };

    if content_lines.is_empty() {
        content_lines.push(Vec::new());
    }
    let limit = max_lines.max(1);
    if content_lines.len() > limit {
        let keep = limit.saturating_sub(1).max(1);
        let hidden = content_lines.len().saturating_sub(keep);
        content_lines.truncate(keep);
        if limit > 1 {
            content_lines.push(vec![Span::styled(
                format!(
                    "\u{2026} (+{hidden} more line{})",
                    if hidden == 1 { "" } else { "s" }
                ),
                Style::default().fg(colour::dim()),
            )]);
        }
    }

    let box_width = room.saturating_sub(4);
    content_lines
        .into_iter()
        .map(|spans| {
            let mut row = Vec::with_capacity(spans.len() + 5);
            row.push(Span::raw("  "));
            if room >= 4 {
                row.push(Span::styled("\u{2503}", Style::default().fg(bar_fg).bg(bg)));
                row.push(Span::styled(" ", Style::default().bg(bg)));
            }
            let painted: Vec<Span<'static>> = spans
                .into_iter()
                .map(|span| Span::styled(span.content, span.style.bg(bg)))
                .collect();
            let body = clip(painted, box_width.saturating_sub(2));
            let body_used: usize = body.iter().map(Span::width).sum();
            row.extend(body);
            let pad = box_width.saturating_sub(2 + body_used);
            if pad > 0 {
                row.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
            }
            Line::from(fill(clip(row, room), width, Color::Reset))
        })
        .collect()
}

fn render_choices(picker: &Picker, decision: &Decision, width: u16) -> Vec<Line<'static>> {
    let room = usize::from(width);
    let max_label = room.saturating_sub(10).clamp(8, 32);
    let label_width = picker
        .choices
        .iter()
        .map(|choice| wrap::columns(&choice.value))
        .max()
        .unwrap_or(0)
        .min(max_label);

    picker
        .choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let selected = index == picker.selected;
            let tone = decision.tone_of(&choice.value);
            let bg = if selected {
                colour::menu_selected_bg()
            } else {
                Color::Reset
            };
            let on = |style: Style| style.bg(bg);

            let marker_fg = if tone == Tone::Danger {
                colour::error()
            } else {
                colour::accent()
            };
            let mut spans = vec![Span::styled(
                if selected {
                    glyph::marker()
                } else {
                    glyph::no_marker()
                },
                on(Style::default().fg(marker_fg)),
            )];

            if decision.quick_keys && index < 9 {
                let num_style = if selected {
                    Style::default()
                        .fg(if tone == Tone::Danger {
                            colour::error()
                        } else {
                            colour::menu_selected()
                        })
                        .add_modifier(Modifier::BOLD)
                } else if tone == Tone::Danger {
                    Style::default().fg(colour::error())
                } else {
                    Style::default().fg(colour::accent())
                };
                spans.push(Span::styled(format!("{}  ", index + 1), on(num_style)));
            }

            let label_style = if selected {
                Style::default()
                    .fg(if tone == Tone::Danger {
                        colour::error()
                    } else {
                        colour::menu_selected()
                    })
                    .add_modifier(Modifier::BOLD)
            } else if !choice.ready {
                Style::default().fg(colour::menu_detail())
            } else if tone == Tone::Danger {
                Style::default().fg(colour::error())
            } else {
                Style::default().fg(colour::text())
            };
            spans.push(Span::styled(choice.value.clone(), on(label_style)));

            if !choice.detail.is_empty() {
                let gap = label_width.saturating_sub(wrap::columns(&choice.value)) + 3;
                let detail_style = if !choice.ready || tone == Tone::Broad {
                    Style::default().fg(colour::warning())
                } else if selected {
                    Style::default().fg(colour::menu_detail_selected())
                } else {
                    Style::default().fg(colour::menu_detail())
                };
                spans.push(Span::styled(" ".repeat(gap), on(Style::default())));
                spans.push(Span::styled(choice.detail.clone(), on(detail_style)));
            }

            Line::from(fill(clip(spans, room), width, bg))
        })
        .collect()
}

fn blank(width: u16) -> Line<'static> {
    Line::from(Span::raw(" ".repeat(usize::from(width))))
}

fn clip(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0usize;
    for span in spans {
        let len = span.width();
        if used + len <= width {
            used += len;
            out.push(span);
            continue;
        }
        let mut left = width.saturating_sub(used);
        if left > 0 {
            let cut: String = span
                .content
                .chars()
                .take_while(|c| {
                    let cols = wrap::columns(c.encode_utf8(&mut [0; 4]));
                    if cols > left {
                        return false;
                    }
                    left -= cols;
                    true
                })
                .collect();
            out.push(Span::styled(cut, span.style));
        }
        break;
    }
    out
}

fn fill(mut spans: Vec<Span<'static>>, width: u16, bg: Color) -> Vec<Span<'static>> {
    let used: usize = spans.iter().map(Span::width).sum();
    let room = usize::from(width).saturating_sub(used);
    if room > 0 {
        spans.push(Span::styled(" ".repeat(room), Style::default().bg(bg)));
    }
    spans
}
