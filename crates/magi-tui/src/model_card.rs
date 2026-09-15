//! The model's card, opened from its name in the footer: what the model is, the settings that can be
//! changed from here, how full this agent's window is, what is published about the model, and who
//! serves it at what price. What was spent is the cost view's. Every part is its own section, set
//! off by a dashed rule. Rows for a list float that scrolls; the rows naming a choice are selectable.

pub(crate) mod charts;
mod published;

use crate::footer::format_tokens;
use charts::Item;
use magi_proto::Usage;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Every reasoning level, lowest first: what ←/→ step along.
pub const LEVELS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "max"];

/// What is published about a model, where anything is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    pub description: Option<String>,
    pub knowledge_cutoff: Option<String>,
    pub modality: Option<String>,
    pub max_output: Option<u64>,
    /// Dollars per million tokens: input, output, cache read, cache write. Zero is unpriced.
    pub price: [f64; 4],
    /// Named scores out of a hundred.
    pub benchmarks: Vec<(String, f64)>,
    pub endpoints: Vec<Endpoint>,
    pub tokenizer: Option<String>,
    /// What it takes in and gives back: `text`, `image`, `audio`.
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// The request parameters it accepts, as the provider names them.
    pub features: Vec<String>,
    /// When it was published, in seconds since the epoch.
    pub created: Option<u64>,
    pub moderated: Option<bool>,
}

/// One provider serving the model, and on what terms.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Endpoint {
    pub provider: String,
    /// What a request names it by; `None` for one that cannot be asked for.
    pub tag: Option<String>,
    pub quantization: Option<String>,
    /// Dollars per million tokens, as [`Details::price`].
    pub price: [f64; 4],
    pub context: Option<u64>,
    pub max_output: Option<u64>,
    pub latency_ms: Option<f64>,
    /// Tokens a second.
    pub throughput: Option<f64>,
}

/// Where the published details stand.
pub enum Known<'a> {
    Asking,
    Missing(&'a str),
    Found(&'a Details),
}

/// What a card is drawn from.
pub struct Card<'a> {
    /// `provider/model`, as the catalog names it.
    pub model: &'a str,
    pub context_window: u64,
    pub reasons: bool,
    pub thinking: &'a str,
    /// Which provider serves it, by tag; `None` leaves it to the router.
    pub provider: Option<&'a str>,
    /// This session's turns, oldest first.
    pub turns: &'a [Usage],
    pub details: Known<'a>,
    /// What the last request was made of, in a line, once one has been laid out.
    pub sent: Option<&'a str>,
    /// Columns the card may take.
    pub width: u16,
}

/// The card's rows and, parallel to them, the choice each one selects.
#[derive(Default)]
pub struct Rendered {
    pub rows: Vec<Line<'static>>,
    pub picks: Vec<Option<String>>,
}

impl Rendered {
    pub(crate) fn push(&mut self, line: Line<'static>, pick: Option<&str>) {
        self.rows.push(line);
        self.picks.push(pick.map(ToOwned::to_owned));
    }

    pub(crate) fn say(&mut self, text: impl Into<String>, style: Style) {
        self.push(Line::from(Span::styled(text.into(), style)), None);
    }

    pub(crate) fn blank(&mut self) {
        self.push(Line::default(), None);
    }

    pub(crate) fn chart(&mut self, lines: Vec<Line<'static>>) {
        for line in lines {
            self.push(line, None);
        }
    }

    /// A dashed rule across the card, then the section's title and what it says about itself.
    pub(crate) fn section(&mut self, title: &str, note: &str, width: u16) {
        self.blank();
        let rule = "- ".repeat(usize::from(width) / 2);
        self.say(
            rule.trim_end(),
            Style::default().fg(crate::colour::border()),
        );
        let mut spans = vec![Span::styled(
            title.to_owned(),
            Style::default()
                .fg(crate::colour::text())
                .add_modifier(Modifier::BOLD),
        )];
        if !note.is_empty() {
            spans.push(Span::styled(
                format!("  {note}"),
                Style::default().fg(crate::colour::dim()),
            ));
        }
        self.push(Line::from(spans), None);
        self.blank();
    }

    /// A label and its value, the labels in one column.
    pub(crate) fn fact(&mut self, label: &str, value: impl Into<String>, ink: &Ink) {
        self.push(
            Line::from(vec![
                Span::styled(format!("{label:<14}"), ink.label),
                Span::raw(value.into()),
            ]),
            None,
        );
    }
}

/// The styles a card is drawn in, shared with the cost view so the two read as one family.
pub(crate) struct Ink {
    pub(crate) dim: Style,
    pub(crate) label: Style,
    pub(crate) value: Style,
}

pub(crate) fn ink() -> Ink {
    Ink {
        dim: Style::default().fg(crate::colour::dim()),
        label: Style::default().fg(crate::colour::muted()),
        value: Style::default().fg(crate::colour::accent()),
    }
}

/// The whole card, top to bottom.
#[must_use]
pub fn view(card: &Card<'_>) -> Rendered {
    let ink = ink();
    let width = card.width.max(20);
    let mut out = Rendered::default();
    heading(&mut out, card, &ink, width);
    out.section("Settings", "", width);
    settings(&mut out, card, &ink);
    context(&mut out, card, width);
    published::sections(&mut out, card, &ink, width);
    out
}

/// The name, where it comes from, and what is said it is for.
fn heading(out: &mut Rendered, card: &Card<'_>, ink: &Ink, width: u16) {
    let (provider, name) = card.model.split_once('/').unwrap_or(("", card.model));
    out.say(name, ink.value.add_modifier(Modifier::BOLD));
    let mut about = Vec::new();
    if !provider.is_empty() {
        about.push(provider.to_owned());
    }
    if card.context_window > 0 {
        about.push(format!("{} context", format_tokens(card.context_window)));
    }
    about.push(
        if card.reasons {
            "reasons"
        } else {
            "no reasoning"
        }
        .to_owned(),
    );
    out.say(about.join(" · "), ink.dim);
    if let Known::Found(Details {
        description: Some(said),
        ..
    }) = card.details
    {
        out.blank();
        for line in crate::wrap::line(Line::from(said.clone()), width)
            .into_iter()
            .take(5)
        {
            out.say(line.to_string(), ink.dim.add_modifier(Modifier::ITALIC));
        }
    }
}

/// The rows that can be taken: the reasoning level, stepped in place, and a way to another model.
fn settings(out: &mut Rendered, card: &Card<'_>, ink: &Ink) {
    let mut thinking = vec![Span::styled(format!("{:<14}", "Thinking"), ink.label)];
    if card.reasons {
        thinking.extend([
            Span::styled("◂ ".to_owned(), ink.dim),
            Span::styled(card.thinking.to_owned(), ink.value),
            Span::styled(" ▸".to_owned(), ink.dim),
        ]);
    } else {
        thinking.push(Span::styled(
            "off — this model does not reason".to_owned(),
            ink.dim,
        ));
    }
    out.push(Line::from(thinking), Some("thinking"));
    out.push(
        Line::from(vec![
            Span::styled(format!("{:<14}", "Model"), ink.label),
            Span::styled("switch to another  ⏎".to_owned(), ink.value),
        ]),
        Some("switch"),
    );
}

/// How full the window is, off the last turn, and what the last request was made of.
fn context(out: &mut Rendered, card: &Card<'_>, width: u16) {
    let used = card.turns.last().map_or(0, |turn| turn.prompt_tokens());
    if card.context_window == 0 || (used == 0 && card.sent.is_none()) {
        return;
    }
    out.section("Context", "how full the window is now", width);
    if used > 0 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a share of the window, to the percent"
        )]
        let share = used as f64 / card.context_window as f64;
        let ink = match share {
            s if s > 0.9 => crate::colour::error(),
            s if s > 0.7 => crate::colour::warning(),
            _ => crate::colour::success(),
        };
        let label = format!(
            "context {:.0}% of {}",
            share * 100.0,
            format_tokens(card.context_window)
        );
        out.chart(charts::gauge(share, &label, ink, width));
    }
    if let Some(sent) = card.sent {
        out.blank();
        out.fact("Sent", sent, &ink());
        out.say("  :context shows how it was laid out", ink().dim);
    }
}

#[cfg(test)]
#[path = "model_card/tests.rs"]
mod tests;
