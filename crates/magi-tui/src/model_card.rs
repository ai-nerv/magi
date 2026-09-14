//! The model's card, opened from its name in the footer: what the model is, the settings that can be
//! changed from here, what its provider publishes about it, and what this session has spent on it,
//! drawn as charts. Rows for a list float that scrolls; the rows naming a setting are selectable.

mod charts;

use crate::footer::format_tokens;
use charts::Item;
use magi_proto::Usage;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Every reasoning level, lowest first: what ←/→ step along.
pub const LEVELS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "max"];

/// What the provider publishes about a model, where it publishes anything.
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
}

/// One provider serving the model, and how reliably it has.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Endpoint {
    pub provider: String,
    pub quantization: Option<String>,
    /// Percent of the last half hour it answered.
    pub uptime: Option<f64>,
}

/// Where the provider's details stand.
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
    /// This session's turns, oldest first.
    pub turns: &'a [Usage],
    pub details: Known<'a>,
    /// Columns the charts may take.
    pub width: u16,
}

/// The card's rows and, parallel to them, the setting each one selects.
#[derive(Default)]
pub struct Rendered {
    pub rows: Vec<Line<'static>>,
    pub picks: Vec<Option<String>>,
}

impl Rendered {
    fn push(&mut self, line: Line<'static>, pick: Option<&str>) {
        self.rows.push(line);
        self.picks.push(pick.map(ToOwned::to_owned));
    }

    fn say(&mut self, text: impl Into<String>, style: Style) {
        self.push(Line::from(Span::styled(text.into(), style)), None);
    }

    fn blank(&mut self) {
        self.push(Line::default(), None);
    }

    fn chart(&mut self, lines: Vec<Line<'static>>) {
        for line in lines {
            self.push(line, None);
        }
    }

    /// A section's title, with a rule running on from it to the width.
    fn section(&mut self, title: &str, width: u16) {
        let rule = usize::from(width).saturating_sub(title.chars().count() + 1);
        self.push(
            Line::from(vec![
                Span::styled(
                    title.to_owned(),
                    Style::default()
                        .fg(crate::colour::muted())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", "─".repeat(rule)),
                    Style::default().fg(crate::colour::border()),
                ),
            ]),
            None,
        );
    }
}

/// The styles a card is drawn in.
struct Ink {
    dim: Style,
    label: Style,
    value: Style,
}

/// The whole card, top to bottom: who the model is, how full its window is, its settings, what the
/// provider says, and this session's spend.
#[must_use]
pub fn view(card: &Card<'_>) -> Rendered {
    let ink = Ink {
        dim: Style::default().fg(crate::colour::dim()),
        label: Style::default().fg(crate::colour::muted()),
        value: Style::default().fg(crate::colour::accent()),
    };
    let width = card.width.max(20);
    let mut out = Rendered::default();
    heading(&mut out, card, &ink, width);
    out.blank();
    settings(&mut out, card, &ink);
    out.blank();
    published(&mut out, &card.details, &ink, width);
    spent(&mut out, card, &ink, width);
    out
}

/// The name, where it comes from, what the provider says it is for, and how full its window is.
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
        for line in crate::wrap::line(Line::from(said.clone()), width)
            .into_iter()
            .take(4)
        {
            out.say(line.to_string(), ink.dim.add_modifier(Modifier::ITALIC));
        }
    }
    let used = card.turns.last().map_or(0, |turn| turn.prompt_tokens());
    if card.context_window > 0 && used > 0 {
        let share = used as f64 / card.context_window as f64;
        let ink = match share {
            s if s > 0.9 => crate::colour::error(),
            s if s > 0.7 => crate::colour::warning(),
            _ => crate::colour::success(),
        };
        out.blank();
        out.chart(charts::gauge(
            share,
            format!(
                "context {:.0}% of {}  ",
                share * 100.0,
                format_tokens(card.context_window)
            ),
            ink,
            width,
        ));
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

/// What the provider publishes: prices, limits, benchmark scores, and who serves it how reliably.
fn published(out: &mut Rendered, known: &Known<'_>, ink: &Ink, width: u16) {
    let details = match known {
        Known::Asking => {
            out.say("asking the provider about it…", ink.dim);
            out.blank();
            return;
        }
        Known::Missing(why) => {
            out.say(*why, ink.dim);
            out.blank();
            return;
        }
        Known::Found(details) => details,
    };
    pricing(out, details, width);
    let facts = [
        (
            "Output",
            details
                .max_output
                .map(|n| format!("up to {} tokens", format_tokens(n))),
        ),
        ("Knows up to", details.knowledge_cutoff.clone()),
        ("Modality", details.modality.clone()),
    ];
    for (label, fact) in facts {
        if let Some(fact) = fact {
            out.push(
                Line::from(vec![
                    Span::styled(format!("{label:<14}"), ink.label),
                    Span::raw(fact),
                ]),
                None,
            );
        }
    }
    out.blank();
    scores(out, details, width);
    serving(out, details, width);
}

/// What a million tokens costs each way, as bars against the dearest.
fn pricing(out: &mut Rendered, details: &Details, width: u16) {
    let [input, output, read, write] = details.price;
    let named = [
        ("input", input, crate::colour::code_command()),
        ("output", output, crate::colour::accent()),
        ("cache read", read, crate::colour::success()),
        ("cache write", write, crate::colour::warning()),
    ];
    let items: Vec<Item> = named
        .iter()
        .filter(|(_, price, _)| *price > 0.0)
        .map(|(label, price, ink)| Item {
            label: (*label).to_owned(),
            value: thousandths(*price),
            said: money(*price),
            ink: *ink,
        })
        .collect();
    if items.is_empty() {
        return;
    }
    out.section("Pricing  per million tokens", width);
    let top = items.iter().map(|item| item.value).max().unwrap_or(1);
    out.chart(charts::bars(&items, top, width));
    out.blank();
}

/// Each benchmark as a bar out of a hundred, each in a colour of its own.
fn scores(out: &mut Rendered, details: &Details, width: u16) {
    if details.benchmarks.is_empty() {
        return;
    }
    let hues = [
        crate::colour::accent(),
        crate::colour::code_command(),
        crate::colour::success(),
        crate::colour::warning(),
    ];
    let items: Vec<Item> = details
        .benchmarks
        .iter()
        .zip(hues.iter().cycle())
        .map(|((name, score), ink)| Item {
            label: name.clone(),
            value: whole(*score),
            said: format!("{score:.1}"),
            ink: *ink,
        })
        .collect();
    out.section("Benchmarks  Artificial Analysis, out of 100", width);
    out.chart(charts::bars(&items, 100, width));
    out.blank();
}

/// Who serves it, most reliable first: the last half hour's uptime, drawn from 90% to 100% so the
/// difference between a good provider and a great one shows.
fn serving(out: &mut Rendered, details: &Details, width: u16) {
    let mut endpoints: Vec<&Endpoint> = details
        .endpoints
        .iter()
        .filter(|endpoint| endpoint.uptime.is_some())
        .collect();
    if endpoints.is_empty() {
        return;
    }
    endpoints.sort_by(|a, b| b.uptime.unwrap_or(0.0).total_cmp(&a.uptime.unwrap_or(0.0)));
    let items: Vec<Item> = endpoints
        .iter()
        .take(8)
        .map(|endpoint| {
            let up = endpoint.uptime.unwrap_or(0.0);
            let mut label = endpoint.provider.clone();
            if let Some(quantization) = &endpoint.quantization {
                label.push_str(&format!(" {quantization}"));
            }
            Item {
                label,
                value: whole((up - 90.0).max(0.0) * 10.0),
                said: format!("{up:.2}%"),
                ink: reliable(up),
            }
        })
        .collect();
    out.section("Providers  uptime, last half hour, 90–100%", width);
    out.chart(charts::bars(&items, 100, width));
    out.blank();
}

/// What this session has spent: the totals, then a column a turn, how full the window got, and
/// the money as it added up.
fn spent(out: &mut Rendered, card: &Card<'_>, ink: &Ink, width: u16) {
    out.section("This session", width);
    let turns = card.turns;
    if turns.is_empty() {
        out.say("nothing yet — this fills in as turns finish", ink.dim);
        return;
    }
    let total = turns.iter().fold(Usage::default(), |mut sum, turn| {
        sum.add(*turn);
        sum
    });
    let mut said = format!(
        "{} turns · {} in · {} out",
        turns.len(),
        format_tokens(total.prompt_tokens()),
        format_tokens(total.output),
    );
    if let Some(rate) = total.cache_hit_rate() {
        said.push_str(&format!(" · {rate:.0}% cached"));
    }
    out.say(said, Style::default());
    if total.cost_micros > 0 {
        out.say(
            format!("{} spent", crate::cost::dollars(total.cost_micros)),
            Style::default().fg(crate::colour::success()),
        );
    }
    out.blank();

    out.say("tokens per turn", ink.label);
    let columns: Vec<Item> = turns
        .iter()
        .enumerate()
        .map(|(at, turn)| {
            let tokens = turn.prompt_tokens() + turn.output;
            Item {
                label: (at + 1).to_string(),
                value: tokens,
                said: format_tokens(tokens),
                ink: crate::colour::code_command(),
            }
        })
        .collect();
    out.chart(charts::columns(&columns, width, 12));
    out.blank();

    if card.context_window > 0 {
        out.say("how full the window got, turn by turn", ink.label);
        let points: Vec<(f64, f64)> = turns
            .iter()
            .enumerate()
            .map(|(at, turn)| {
                (
                    (at + 1) as f64,
                    turn.prompt_tokens() as f64 / card.context_window as f64 * 100.0,
                )
            })
            .collect();
        let ticks = ["0%".to_owned(), "50%".to_owned(), "100%".to_owned()];
        let ink = crate::colour::warning();
        out.chart(charts::line(&points, 100.0, ticks, ink, width, 10));
        out.blank();
    }

    if total.cost_micros > 0 {
        out.say("money, as it added up", ink.label);
        let mut running = 0_u64;
        let points: Vec<(f64, f64)> = turns
            .iter()
            .enumerate()
            .map(|(at, turn)| {
                running += turn.cost_micros;
                ((at + 1) as f64, running as f64)
            })
            .collect();
        let top = total.cost_micros as f64;
        let ticks = [
            "$0".to_owned(),
            crate::cost::dollars(total.cost_micros / 2),
            crate::cost::dollars(total.cost_micros),
        ];
        let ink = crate::colour::success();
        out.chart(charts::line(&points, top, ticks, ink, width, 10));
    }
}

/// Green for a provider that nearly always answers, orange for one that mostly does, red otherwise.
fn reliable(uptime: f64) -> Color {
    if uptime >= 99.0 {
        crate::colour::success()
    } else if uptime >= 95.0 {
        crate::colour::warning()
    } else {
        crate::colour::error()
    }
}

/// A price per million tokens: to the cent, or finer for one below a cent.
fn money(dollars: f64) -> String {
    if dollars >= 0.01 || dollars == 0.0 {
        format!("${dollars:.2}")
    } else {
        format!("${dollars:.4}")
    }
}

/// A price in thousandths of a dollar, so bars can be compared in whole numbers.
fn thousandths(dollars: f64) -> u64 {
    whole(dollars * 1000.0)
}

/// A non-negative figure rounded to a whole number.
fn whole(value: f64) -> u64 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to zero and rounded first"
    )]
    let rounded = value.max(0.0).round() as u64;
    rounded
}

#[cfg(test)]
#[path = "model_card/tests.rs"]
mod tests;
