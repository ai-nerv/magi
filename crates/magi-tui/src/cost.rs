//! What this session has spent, and on what.
//!
//! **Four counters, because a provider bills four things.** Input, output, cache read and cache
//! write are priced differently by every provider that offers caching, and a single "tokens"
//! number hides the one decision a person can act on: a session whose prompt is mostly cache
//! reads is cheap to keep going, and one whose prompt is mostly fresh input is not.
//!
//! **No money.** magi does not know what a token costs — melchior owns the catalog, and the
//! per-million rates live in its `providers.lua` beside the endpoint they belong to. Guessing
//! here would be a number that goes stale the day a provider changes its price, printed with the
//! authority of one that did not. The view says what was spent and says where the rate lives.

use crate::footer::format_tokens;
use magi_proto::Usage;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One turn's spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    /// Which turn, counting from one.
    pub at: usize,
    /// What it used.
    pub usage: Usage,
}

/// The rows of a cost view, `width` columns wide.
///
/// Newest last, like the transcript: a session is read downwards and a spend table that ran the
/// other way would be the one thing on screen that did.
#[must_use]
pub fn lines(turns: &[Turn], model: Option<&str>) -> Vec<Line<'static>> {
    if turns.is_empty() {
        return Vec::new();
    }
    let total = turns.iter().fold(Usage::default(), |sum, turn| Usage {
        input: sum.input + turn.usage.input,
        output: sum.output + turn.usage.output,
        cache_read: sum.cache_read + turn.usage.cache_read,
        cache_write: sum.cache_write + turn.usage.cache_write,
    });

    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![
        Line::from(Span::styled(
            format!(
                "{:<6}{:>10}{:>10}{:>12}{:>12}",
                "turn", "in", "out", "cache rd", "cache wr"
            ),
            dim,
        )),
        Line::from(String::new()),
    ];

    for turn in turns {
        out.push(Line::from(format!(
            "{:<6}{:>10}{:>10}{:>12}{:>12}",
            turn.at,
            format_tokens(turn.usage.input),
            format_tokens(turn.usage.output),
            format_tokens(turn.usage.cache_read),
            format_tokens(turn.usage.cache_write),
        )));
    }

    out.push(Line::from(String::new()));
    out.push(Line::from(Span::styled(
        format!(
            "{:<6}{:>10}{:>10}{:>12}{:>12}",
            "all",
            format_tokens(total.input),
            format_tokens(total.output),
            format_tokens(total.cache_read),
            format_tokens(total.cache_write),
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    // **The one derived number worth printing.** Everything else here is a tally; this is the
    // ratio that says whether the conversation is getting cheaper or more expensive to continue.
    let prompt = total.prompt_tokens();
    if prompt > 0 {
        let served = total.cache_read * 100 / prompt;
        out.push(Line::from(Span::styled(
            format!("      {served}% of the prompt was served from cache"),
            dim,
        )));
    }

    out.push(Line::from(String::new()));
    out.push(Line::from(Span::styled(
        match model {
            Some(model) => format!("rates for {model} live in melchior's providers.lua"),
            None => "rates live in melchior's providers.lua".to_owned(),
        },
        dim,
    )));
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
