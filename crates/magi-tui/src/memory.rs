//! balthasar's float: what the memory layer holds, what of it this model is being told, and what
//! any of it has been worth. One tab per question, because they are four different questions and a
//! single list that answered all of them would answer none.

use crate::model_card::Rendered;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The tabs, in the order the strip draws them.
pub const TABS: [&str; 4] = ["sections", "memories", "evidence", "sessions"];

/// What the float draws from: each tab's answer as the memory layer last gave it.
pub struct Held<'a> {
    /// How the last request was laid out — the sections tab is this and nothing else.
    pub laid: Option<&'a crate::laid::Laid>,
    pub jobs: &'a [crate::cost::Helper],
    /// `None` until the layer has answered; empty once it has and there is nothing.
    pub memories: Option<&'a [serde_json::Value]>,
    /// The memory the cursor was on when `utility` and `why` were asked for, and their answers.
    pub chosen: Option<&'a str>,
    pub utility: Option<&'a serde_json::Value>,
    pub why: Option<&'a serde_json::Value>,
    pub sessions: Option<&'a [serde_json::Value]>,
    pub width: u16,
}

/// What to say with no rows to show. `answered` is whether the layer has replied: a store that
/// holds nothing and a question still in flight look the same on screen otherwise, and the empty
/// one then reads as a hang.
#[must_use]
pub fn empty(tab: usize, answered: bool) -> String {
    match TABS.get(tab) {
        Some(&"sections") => crate::laid::empty(),
        Some(&"evidence") => "pick a memory on the memories tab to see what it rests on".into(),
        Some(&"memories") if answered => "this project has taught it nothing yet".into(),
        Some(&"sessions") if answered => "no run here has been recorded".into(),
        _ => "asking the memory layer…".to_owned(),
    }
}

/// Draw one tab.
#[must_use]
pub fn view(held: &Held<'_>, tab: usize) -> Rendered {
    match TABS.get(tab) {
        Some(&"sections") => held
            .laid
            .map(|laid| crate::laid::view(laid, held.jobs, held.width))
            .unwrap_or_default(),
        Some(&"memories") => memories(held),
        Some(&"evidence") => evidence(held),
        Some(&"sessions") => sessions(held),
        _ => Rendered::default(),
    }
}

fn dim() -> Style {
    Style::default().fg(crate::colour::dim())
}

fn said<'a>(row: &'a serde_json::Value, key: &str) -> &'a str {
    row[key].as_str().unwrap_or_default()
}

/// A heading with a dashed rule under it, the way every other card sets a section off.
fn rule(out: &mut Rendered, title: &str, width: u16) {
    if !out.rows.is_empty() {
        out.push(Line::from(String::new()), None);
    }
    out.push(
        Line::from(Span::styled(
            title.to_owned(),
            Style::default()
                .fg(crate::colour::hint())
                .add_modifier(Modifier::BOLD),
        )),
        None,
    );
    out.push(
        Line::from(Span::styled("─".repeat(usize::from(width).min(64)), dim())),
        None,
    );
}

/// One memory a line: what it says, and how sure the layer is of it. Selectable, because the
/// utility tab is about whichever one the cursor is on.
fn memories(held: &Held<'_>) -> Rendered {
    let mut out = Rendered::default();
    for row in held.memories.unwrap_or_default() {
        let id = said(row, "id");
        let text = said(row, "text");
        let kind = said(row, "kind");
        let score = row["confidence"]
            .as_f64()
            .or_else(|| row["score"].as_f64())
            .map(|n| format!("{n:.2}"))
            .unwrap_or_default();
        let mut spans = Vec::new();
        if !kind.is_empty() {
            spans.push(Span::styled(format!("{kind:<10} "), dim()));
        }
        spans.push(Span::raw(fit(text, held.width.saturating_sub(18))));
        if !score.is_empty() {
            spans.push(Span::styled(format!("  {score}"), dim()));
        }
        out.push(Line::from(spans), Some(id));
    }
    out
}

/// What one memory rests on, and — where the layer offers the counts — what it has been worth.
/// `utility` is balthasar's own rather than the memory role's, so its absence is ordinary: the
/// evidence is what every memory layer owes.
fn evidence(held: &Held<'_>) -> Rendered {
    let mut out = Rendered::default();
    let Some(id) = held.chosen else {
        return out;
    };
    rule(&mut out, id, held.width);

    if let Some(use_of) = held.utility {
        let count = |key: &str| use_of[key].as_u64().unwrap_or(0);
        let considered = count("times_considered");
        let returned = count("times_returned");
        pair(
            &mut out,
            "retrieved",
            &format!("{returned} returned of {considered} considered"),
        );
        if let Some(rate) = use_of["helpfulness"].as_f64() {
            pair(&mut out, "helpfulness", &format!("{:.0}%", rate * 100.0));
        }
        for (label, key) in [
            ("helped", "verified_helpful"),
            ("harmed", "verified_harmful"),
            ("ignored", "ignored"),
            ("unknown", "unknown"),
        ] {
            pair(&mut out, label, &count(key).to_string());
        }
    }

    if let Some(why) = held.why {
        rule(&mut out, "evidence", held.width);
        if let Some(sure) = why["confidence"].as_f64() {
            pair(&mut out, "confidence", &format!("{sure:.2}"));
        }
        if let Some(sessions) = why["sessions"].as_u64() {
            pair(
                &mut out,
                "asserted by",
                &format!("{sessions} session{}", if sessions == 1 { "" } else { "s" }),
            );
        }
        witnesses(&mut out, why, "witnesses", held.width);
        witnesses(&mut out, why, "against", held.width);
    }
    out
}

/// One list of witnesses: when it was said, by whom, and what they were doing at the time.
fn witnesses(out: &mut Rendered, why: &serde_json::Value, key: &str, width: u16) {
    let rows = why[key].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if rows.is_empty() {
        if key == "witnesses" {
            out.push(
                Line::from(Span::styled("nothing has asserted this yet", dim())),
                None,
            );
        }
        return;
    }
    if key != "witnesses" {
        out.push(Line::from(Span::styled("against".to_owned(), dim())), None);
    }
    for row in rows {
        let when = row["at"]
            .as_u64()
            .map(crate::model_card::published::date)
            .unwrap_or_default();
        let note = match said(row, "note") {
            "" => said(row, "kind"),
            note => note,
        };
        out.push(
            Line::from(vec![
                Span::styled(format!("{when:<12}  "), dim()),
                Span::raw(format!("{:<11}", said(row, "session"))),
                Span::styled(fit(note, width.saturating_sub(28)), dim()),
            ]),
            None,
        );
    }
}

/// The runs this project has had, newest first, and whether each is still going.
fn sessions(held: &Held<'_>) -> Rendered {
    let mut out = Rendered::default();
    for row in held.sessions.unwrap_or_default() {
        let title = match said(row, "title") {
            "" => said(row, "name"),
            title => title,
        };
        let open = row["open"].as_bool().unwrap_or(false);
        let mark = if open { "●" } else { " " };
        out.push(
            Line::from(vec![
                Span::styled(
                    format!("{mark} "),
                    if open {
                        Style::default().fg(crate::colour::hint())
                    } else {
                        dim()
                    },
                ),
                Span::raw(fit(title, held.width.saturating_sub(16))),
                Span::styled(format!("  {}", said(row, "harness")), dim()),
            ]),
            Some(said(row, "id")),
        );
    }
    out
}

/// A label and its value, lined up.
fn pair(out: &mut Rendered, label: &str, value: &str) {
    out.push(
        Line::from(vec![
            Span::styled(format!("{label:<14}"), dim()),
            Span::raw(value.to_owned()),
        ]),
        None,
    );
}

/// Cut to `width`, with an ellipsis where anything was lost.
fn fit(text: &str, width: u16) -> String {
    let width = usize::from(width).max(8);
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= width {
        return flat;
    }
    format!("{}…", flat.chars().take(width - 1).collect::<String>())
}

#[cfg(test)]
#[path = "memory/tests.rs"]
mod tests;
