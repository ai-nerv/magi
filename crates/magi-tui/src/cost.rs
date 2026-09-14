//! What this session has spent, and on what. Four token counters because providers price input,
//! output, cache read and cache write differently; and money where the provider said what a request
//! cost, as OpenRouter does. No rate is guessed here: melchior's `providers.lua` owns those.

use crate::footer::format_tokens;
use magi_proto::Usage;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One turn's spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub at: usize,
    pub usage: Usage,
}

/// Millionths of a dollar, as dollars to a hundredth of a cent: one turn often costs less than one.
#[must_use]
pub fn dollars(micros: u64) -> String {
    format!("${}.{:04}", micros / 1_000_000, micros % 1_000_000 / 100)
}

/// The rows of a cost view, newest last like the transcript. The money column is there only once
/// the provider has said what something cost.
#[must_use]
pub fn lines(turns: &[Turn], model: Option<&str>) -> Vec<Line<'static>> {
    if turns.is_empty() {
        return Vec::new();
    }
    let total = turns.iter().fold(Usage::default(), |mut sum, turn| {
        sum.add(turn.usage);
        sum
    });
    let priced = total.cost_micros > 0;
    let row = |at: &str, usage: Usage| {
        let mut said = format!(
            "{at:<6}{:>10}{:>10}{:>12}{:>12}",
            format_tokens(usage.input),
            format_tokens(usage.output),
            format_tokens(usage.cache_read),
            format_tokens(usage.cache_write),
        );
        if priced {
            said.push_str(&format!("{:>12}", dollars(usage.cost_micros)));
        }
        said
    };

    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut heading = format!(
        "{:<6}{:>10}{:>10}{:>12}{:>12}",
        "turn", "in", "out", "cache rd", "cache wr"
    );
    if priced {
        heading.push_str(&format!("{:>12}", "cost"));
    }
    let mut out = vec![
        Line::from(Span::styled(heading, dim)),
        Line::from(String::new()),
    ];
    for turn in turns {
        out.push(Line::from(row(&turn.at.to_string(), turn.usage)));
    }
    out.push(Line::from(String::new()));
    out.push(Line::from(Span::styled(
        row("all", total),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    let prompt = total.prompt_tokens();
    if prompt > 0 {
        let served = total.cache_read * 100 / prompt;
        out.push(Line::from(Span::styled(
            format!("      {served}% of the prompt was served from cache"),
            dim,
        )));
    }

    out.push(Line::from(String::new()));
    let closing = match (priced, model) {
        (true, _) => format!(
            "spent {} this session, as the provider reported it",
            dollars(total.cost_micros)
        ),
        (false, Some(model)) => format!("rates for {model} live in melchior's providers.lua"),
        (false, None) => "rates live in melchior's providers.lua".to_owned(),
    };
    out.push(Line::from(Span::styled(closing, dim)));
    out
}

/// What to say when nothing has been spent.
#[must_use]
pub fn empty() -> String {
    "nothing spent yet — this fills in as turns finish".to_owned()
}

#[cfg(test)]
#[path = "cost/tallying.rs"]
mod tallying;
