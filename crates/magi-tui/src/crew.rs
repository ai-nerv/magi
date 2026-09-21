//! melchior's float: who is in this run, which models they have reached, and what each has spent.
//!
//! The roster is melchior's own answer, already pushed here for the agents view; this is the same
//! rows asked three different questions.

use crate::model_card::Rendered;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub const TABS: [&str; 3] = ["crew", "models", "spend"];

/// One agent, as the roster carries it.
pub struct Agent<'a> {
    pub name: &'a str,
    pub role: &'a str,
    pub here: bool,
    pub phase: &'a str,
    pub claim: Option<&'a str>,
    /// A row a model: what this agent spent talking to it.
    pub spent: &'a [(String, magi_proto::Usage)],
}

pub struct Held<'a> {
    pub agents: &'a [Agent<'a>],
    pub width: u16,
}

#[must_use]
pub fn empty(tab: usize) -> String {
    match TABS.get(tab) {
        Some(&"crew") => "nobody else is in this run".to_owned(),
        _ => "nothing has been spent in this run yet".to_owned(),
    }
}

#[must_use]
pub fn view(held: &Held<'_>, tab: usize) -> Rendered {
    match TABS.get(tab) {
        Some(&"crew") => crew(held),
        Some(&"models") => models(held),
        Some(&"spend") => spend(held),
        _ => Rendered::default(),
    }
}

fn dim() -> Style {
    Style::default().fg(crate::colour::dim())
}

/// Who is here, what each is for, and what it is doing now.
fn crew(held: &Held<'_>) -> Rendered {
    let mut out = Rendered::default();
    for agent in held.agents {
        let mark = if agent.here { "▸ " } else { "  " };
        let mut spans = vec![
            Span::styled(mark.to_owned(), dim()),
            Span::raw(format!("{:<22}", agent.name)),
            Span::styled(format!("{:<10}", agent.role), dim()),
            Span::styled(agent.phase.to_owned(), dim()),
        ];
        if let Some(claim) = agent.claim.filter(|claim| !claim.is_empty()) {
            spans.push(Span::styled(
                format!("  {}", fit(claim, held.width.saturating_sub(46))),
                Style::default().fg(crate::colour::hint()),
            ));
        }
        out.push(Line::from(spans), Some(agent.name));
    }
    out
}

/// Which models this run has reached, and what each cost — the whole run, not this agent.
fn models(held: &Held<'_>) -> Rendered {
    let mut totals: std::collections::BTreeMap<&str, (magi_proto::Usage, usize)> =
        std::collections::BTreeMap::new();
    for agent in held.agents {
        for (model, used) in agent.spent {
            let row = totals.entry(model.as_str()).or_default();
            row.0 = added(row.0, *used);
            row.1 += 1;
        }
    }
    let mut out = Rendered::default();
    for (model, (used, agents)) in &totals {
        out.push(
            Line::from(vec![
                Span::raw(format!("{:<34}", fit(model, 33))),
                Span::styled(format!("{:>10}", tokens(used)), dim()),
                Span::styled(format!("{:>10}", money(used.cost_micros)), dim()),
                Span::styled(
                    format!("  {agents} agent{}", if *agents == 1 { "" } else { "s" }),
                    dim(),
                ),
            ]),
            None,
        );
    }
    out
}

/// What each agent spent, model by model: who is calling, and how much.
fn spend(held: &Held<'_>) -> Rendered {
    let mut out = Rendered::default();
    for agent in held.agents {
        if agent.spent.is_empty() {
            continue;
        }
        let total = agent
            .spent
            .iter()
            .fold(magi_proto::Usage::default(), |sum, (_, used)| {
                added(sum, *used)
            });
        out.push(
            Line::from(vec![
                Span::styled(
                    format!("{:<24}", agent.name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:>10}", tokens(&total)), dim()),
                Span::styled(format!("{:>10}", money(total.cost_micros)), dim()),
            ]),
            Some(agent.name),
        );
        for (model, used) in agent.spent {
            out.push(
                Line::from(vec![
                    Span::styled(format!("  {:<22}", fit(model, 21)), dim()),
                    Span::styled(format!("{:>10}", tokens(used)), dim()),
                    Span::styled(format!("{:>10}", money(used.cost_micros)), dim()),
                ]),
                Some(agent.name),
            );
        }
    }
    out
}

fn added(a: magi_proto::Usage, b: magi_proto::Usage) -> magi_proto::Usage {
    magi_proto::Usage {
        input: a.input + b.input,
        output: a.output + b.output,
        cache_read: a.cache_read + b.cache_read,
        cache_write: a.cache_write + b.cache_write,
        cost_micros: a.cost_micros + b.cost_micros,
    }
}

/// Everything that went over the wire, in and out.
fn tokens(used: &magi_proto::Usage) -> String {
    crate::footer::format_tokens(used.prompt_tokens() + used.output)
}

/// Micros as a person reads them; a run that has cost nothing says so with a dash.
fn money(micros: u64) -> String {
    if micros == 0 {
        return "—".to_owned();
    }
    let dollars = micros as f64 / 1_000_000.0;
    if dollars < 0.01 {
        return "<$0.01".to_owned();
    }
    format!("${dollars:.2}")
}

fn fit(text: &str, width: u16) -> String {
    let width = usize::from(width).max(8);
    if text.chars().count() <= width {
        return text.to_owned();
    }
    format!("{}…", text.chars().take(width - 1).collect::<String>())
}

#[cfg(test)]
#[path = "crew/tests.rs"]
mod tests;
