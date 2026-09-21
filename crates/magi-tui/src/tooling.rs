//! casper's float: what it offers, and what this session has actually reached for.
//!
//! The counts are read off the transcript this session already holds rather than kept anywhere:
//! balthasar is the one store, and a tally beside it would be a second that goes stale.

use crate::model_card::Rendered;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub const TABS: [&str; 2] = ["tools", "calls"];

/// One tool, as the program filling the `tools` role describes it. Owned, because it is read off
/// that program on a thread of its own and handed over whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub group: String,
    pub needs: Option<String>,
    pub deferred: bool,
    pub about: String,
}

pub struct Held<'a> {
    pub tools: Option<&'a [Tool]>,
    /// How often each tool was called this session, most used first.
    pub calls: &'a [(String, usize)],
    pub width: u16,
}

#[must_use]
pub fn empty(tab: usize, answered: bool) -> String {
    match TABS.get(tab) {
        Some(&"calls") => "no tool has been called in this session yet".to_owned(),
        _ if answered => "this session has no tools".to_owned(),
        _ => "asking casper what it offers…".to_owned(),
    }
}

#[must_use]
pub fn view(held: &Held<'_>, tab: usize) -> Rendered {
    match TABS.get(tab) {
        Some(&"tools") => tools(held),
        Some(&"calls") => calls(held),
        _ => Rendered::default(),
    }
}

fn dim() -> Style {
    Style::default().fg(crate::colour::dim())
}

/// Every tool, under the branch of the manual it belongs to, with the permission it acts under.
fn tools(held: &Held<'_>) -> Rendered {
    let all = held.tools.unwrap_or_default();
    // Gathered rather than run through in order: the branches arrive interleaved, and a heading
    // raised whenever the group changes prints `files` twice.
    let mut branches: Vec<&str> = Vec::new();
    for tool in all {
        let group = named(tool);
        if !branches.contains(&group) {
            branches.push(group);
        }
    }
    let mut out = Rendered::default();
    let mut first = true;
    for branch in branches {
        {
            if !first {
                out.push(Line::from(String::new()), None);
            }
            first = false;
            let group = branch;
            out.push(
                Line::from(Span::styled(
                    group.to_owned(),
                    Style::default()
                        .fg(crate::colour::hint())
                        .add_modifier(Modifier::BOLD),
                )),
                None,
            );
        }
        for tool in all.iter().filter(|tool| named(tool) == branch) {
            let mut spans = vec![
                Span::raw(format!("  {:<10}", tool.name)),
                Span::styled(
                    format!("{:<7}", tool.needs.as_deref().unwrap_or("—")),
                    dim(),
                ),
            ];
            if tool.deferred {
                spans.push(Span::styled("deferred  ".to_owned(), dim()));
            }
            spans.push(Span::styled(
                fit(&tool.about, held.width.saturating_sub(32)),
                dim(),
            ));
            out.push(Line::from(spans), Some(&tool.name));
        }
    }
    out
}

/// The branch a tool sits under; one that names none has a home of its own.
fn named(tool: &Tool) -> &str {
    if tool.group.is_empty() {
        "other"
    } else {
        tool.group.as_str()
    }
}

/// What this session reached for, most used first, with a bar so the shape reads at a glance.
fn calls(held: &Held<'_>) -> Rendered {
    let mut out = Rendered::default();
    let most = held.calls.iter().map(|(_, n)| *n).max().unwrap_or(0);
    let total: usize = held.calls.iter().map(|(_, n)| *n).sum();
    let room = usize::from(held.width.saturating_sub(28)).clamp(4, 40);
    for (name, count) in held.calls {
        let filled = if most == 0 { 0 } else { count * room / most };
        out.push(
            Line::from(vec![
                Span::raw(format!("{name:<12}")),
                Span::styled(format!("{count:>5}  "), dim()),
                Span::styled(
                    "▂".repeat(filled),
                    Style::default().fg(crate::colour::hint()),
                ),
            ]),
            Some(name),
        );
    }
    if total > 0 {
        out.push(Line::from(String::new()), None);
        out.push(
            Line::from(Span::styled(
                format!(
                    "({total} call{}, {} tool{})",
                    if total == 1 { "" } else { "s" },
                    held.calls.len(),
                    if held.calls.len() == 1 { "" } else { "s" }
                ),
                dim(),
            )),
            None,
        );
    }
    out
}

fn fit(text: &str, width: u16) -> String {
    let width = usize::from(width).max(8);
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= width {
        return flat;
    }
    format!("{}…", flat.chars().take(width - 1).collect::<String>())
}

#[cfg(test)]
#[path = "tooling/tests.rs"]
mod tests;
