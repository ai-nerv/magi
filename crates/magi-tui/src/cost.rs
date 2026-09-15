//! What was spent, and on what: this agent's turns, each model they went to, and every agent of the
//! run it belongs to. Four token counters, because providers price input, output and cache apart,
//! and money wherever the provider said what a request cost. No rate is guessed here: melchior's
//! `providers.lua` owns them. Drawn the way the model's card is, a section each.

use crate::footer::format_tokens;
use crate::model_card::charts::{self, Item};
use crate::model_card::{Ink, Rendered};
use magi_proto::Usage;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// One finished turn of this agent: which it was, which model answered it, and what it took.
#[derive(Debug, Clone)]
pub struct Turn {
    pub at: usize,
    pub model: String,
    pub usage: Usage,
}

/// One agent of the run, and what each model has cost it.
#[derive(Debug, Clone, Default)]
pub struct Agent {
    pub name: String,
    /// This screen's own.
    pub here: bool,
    pub spent: Vec<(String, Usage)>,
}

/// What a cost view is drawn from.
pub struct Report<'a> {
    pub turns: &'a [Turn],
    /// Every agent of the run, this one included; empty or alone when there are no others.
    pub agents: &'a [Agent],
    pub width: u16,
}

/// How many of the newest turns are listed.
const RECENT: usize = 10;

/// Millionths of a dollar, as dollars to a hundredth of a cent: one turn often costs less than one.
#[must_use]
pub fn dollars(micros: u64) -> String {
    format!("${}.{:04}", micros / 1_000_000, micros % 1_000_000 / 100)
}

/// What to say when nothing has been spent.
#[must_use]
pub fn empty() -> String {
    "nothing spent yet — this fills in as turns finish".to_owned()
}

/// The whole view, top to bottom: nothing at all when nothing was spent.
#[must_use]
pub fn view(report: &Report<'_>) -> Rendered {
    let ink = crate::model_card::ink();
    let width = report.width.max(20);
    let mut out = Rendered::default();
    let mine = sum(report.turns.iter().map(|turn| turn.usage));
    let run = report.agents.len() > 1;
    let whole = if run {
        sum(report
            .agents
            .iter()
            .flat_map(|agent| agent.spent.iter().map(|(_, used)| *used)))
    } else {
        mine
    };
    if tokens(&whole) == 0 && whole.cost_micros == 0 {
        return out;
    }
    let priced = whole.cost_micros > 0;
    // Each model in the order it first appears, so it keeps one colour from section to section.
    let mut models: Vec<String> = Vec::new();
    let named = report.turns.iter().map(|turn| turn.model.clone()).chain(
        report
            .agents
            .iter()
            .flat_map(|agent| agent.spent.iter().map(|(model, _)| model.clone())),
    );
    for model in named {
        if !models.contains(&model) {
            models.push(model);
        }
    }
    let colour = |model: &str| hue(models.iter().position(|known| known == model).unwrap_or(0));

    heading(
        &mut out,
        report,
        (&mine, &whole),
        priced,
        models.len(),
        &ink,
    );
    this_agent(&mut out, &mine, report.turns.len(), &ink, width);
    by_model(&mut out, report, run, priced, &colour, width);
    if run {
        by_agent(&mut out, report, priced, width);
    }
    per_turn(&mut out, report, &colour, width);
    over_time(&mut out, report, priced, width);
    cached(&mut out, &mine, width);
    recent(&mut out, report, priced, &ink, &colour, width);
    out
}

/// What was spent in all, large, and what it was spread over.
fn heading(
    out: &mut Rendered,
    report: &Report<'_>,
    (mine, whole): (&Usage, &Usage),
    priced: bool,
    models: usize,
    ink: &Ink,
) {
    let (big, ink_big) = if priced {
        (
            format!("{} spent", dollars(whole.cost_micros)),
            crate::colour::success(),
        )
    } else {
        (
            format!("{} tokens", format_tokens(tokens(whole))),
            crate::colour::accent(),
        )
    };
    out.say(
        big,
        Style::default().fg(ink_big).add_modifier(Modifier::BOLD),
    );
    let mut about = Vec::new();
    if report.agents.len() > 1 {
        about.push(format!("the whole run, {} agents", report.agents.len()));
        let here = if priced {
            dollars(mine.cost_micros)
        } else {
            format_tokens(tokens(mine))
        };
        about.push(format!("this agent {here}"));
    } else {
        about.push("this agent".to_owned());
    }
    about.push(format!("{} turns", report.turns.len()));
    about.push(format!(
        "{models} model{}",
        if models == 1 { "" } else { "s" }
    ));
    out.say(about.join(" · "), ink.dim);
}

/// This agent's own totals, counter by counter.
fn this_agent(out: &mut Rendered, mine: &Usage, turns: usize, ink: &Ink, width: u16) {
    out.section("This agent", "", width);
    out.fact("Turns", turns.to_string(), ink);
    out.fact(
        "Tokens",
        format!(
            "{} in · {} out",
            format_tokens(mine.prompt_tokens()),
            format_tokens(mine.output)
        ),
        ink,
    );
    if mine.cache_read > 0 || mine.cache_write > 0 {
        out.fact(
            "Cache",
            format!(
                "{} read · {} written",
                format_tokens(mine.cache_read),
                format_tokens(mine.cache_write)
            ),
            ink,
        );
    }
    if mine.cost_micros > 0 {
        out.push(
            Line::from(vec![
                Span::styled(format!("{:<14}", "Spent"), ink.label),
                Span::styled(
                    dollars(mine.cost_micros),
                    Style::default().fg(crate::colour::success()),
                ),
            ]),
            None,
        );
    }
}

/// Each model a bar: over the whole run when there is one, over this agent otherwise.
fn by_model(
    out: &mut Rendered,
    report: &Report<'_>,
    run: bool,
    priced: bool,
    colour: &dyn Fn(&str) -> Color,
    width: u16,
) {
    let rows = if run {
        gather(
            report
                .agents
                .iter()
                .flat_map(|agent| agent.spent.iter().cloned()),
        )
    } else {
        gather(
            report
                .turns
                .iter()
                .map(|turn| (turn.model.clone(), turn.usage)),
        )
    };
    if rows.is_empty() {
        return;
    }
    out.section(
        "By model",
        if run { "the whole run" } else { "this agent" },
        width,
    );
    let items: Vec<Item> = rows
        .iter()
        .map(|(model, used)| {
            let (value, said) = amount(used, priced);
            Item {
                label: short(model).to_owned(),
                value,
                said,
                ink: colour(model),
            }
        })
        .collect();
    let top = items.iter().map(|item| item.value).fold(0.0_f64, f64::max);
    out.chart(charts::bars(&items, top, width));
}

/// Each agent of the run a bar, the dearest first.
fn by_agent(out: &mut Rendered, report: &Report<'_>, priced: bool, width: u16) {
    out.section("By agent", "the whole run", width);
    let mut items: Vec<Item> = report
        .agents
        .iter()
        .map(|agent| {
            let used = sum(agent.spent.iter().map(|(_, used)| *used));
            let (value, said) = amount(&used, priced);
            Item {
                label: if agent.here {
                    "you".to_owned()
                } else {
                    agent.name.clone()
                },
                value,
                said,
                ink: if agent.here {
                    crate::colour::accent()
                } else {
                    crate::colour::code_command()
                },
            }
        })
        .collect();
    items.sort_by(|a, b| b.value.total_cmp(&a.value));
    let top = items.iter().map(|item| item.value).fold(0.0_f64, f64::max);
    out.chart(charts::bars(&items, top, width));
}

/// A column a turn, each in the colour of the model that answered it.
fn per_turn(out: &mut Rendered, report: &Report<'_>, colour: &dyn Fn(&str) -> Color, width: u16) {
    if report.turns.is_empty() {
        return;
    }
    out.section(
        "Tokens per turn",
        "this agent, each in its model's colour",
        width,
    );
    let items: Vec<Item> = report
        .turns
        .iter()
        .map(|turn| Item {
            label: turn.at.to_string(),
            value: figure(tokens(&turn.usage)),
            said: format_tokens(tokens(&turn.usage)),
            ink: colour(&turn.model),
        })
        .collect();
    out.chart(charts::columns(&items, width, 8));
}

/// The money as it added up, turn by turn, or the tokens where nothing was priced.
fn over_time(out: &mut Rendered, report: &Report<'_>, priced: bool, width: u16) {
    if report.turns.len() < 2 {
        return;
    }
    if priced {
        out.section("Spend over time", "this agent, as it added up", width);
    } else {
        out.section("Tokens over time", "this agent, as they added up", width);
    }
    let mut running = 0_u64;
    let points: Vec<(f64, f64)> = report
        .turns
        .iter()
        .enumerate()
        .map(|(n, turn)| {
            running += if priced {
                turn.usage.cost_micros
            } else {
                tokens(&turn.usage)
            };
            (figure(n as u64 + 1), figure(running))
        })
        .collect();
    let (ticks, ink) = if priced {
        (
            ["$0".to_owned(), dollars(running / 2), dollars(running)],
            crate::colour::success(),
        )
    } else {
        (
            [
                "0".to_owned(),
                format_tokens(running / 2),
                format_tokens(running),
            ],
            crate::colour::accent(),
        )
    };
    out.chart(charts::line(
        &points,
        figure(running),
        ticks,
        ink,
        width,
        10,
    ));
}

/// How much of what went in was served from cache, as a gauge.
fn cached(out: &mut Rendered, mine: &Usage, width: u16) {
    let prompt = mine.input + mine.cache_read + mine.cache_write;
    if prompt == 0 || mine.cache_read == 0 {
        return;
    }
    let share = figure(mine.cache_read) / figure(prompt);
    out.section("Cache", "this agent", width);
    let label = format!("{:.0}% of the prompt from cache", share * 100.0);
    out.chart(charts::gauge(
        share,
        &label,
        crate::colour::success(),
        width,
    ));
}

/// The newest turns as rows: which, which model, what went in and out, and what it cost.
fn recent(
    out: &mut Rendered,
    report: &Report<'_>,
    priced: bool,
    ink: &Ink,
    colour: &dyn Fn(&str) -> Color,
    width: u16,
) {
    if report.turns.is_empty() {
        return;
    }
    out.section("Recent turns", "newest last", width);
    let head = format!(
        "{:<6}{:<20}{:>9}{:>9}{:>11}",
        "turn",
        "model",
        "in",
        "out",
        if priced { "cost" } else { "" }
    );
    out.say(head.trim_end(), ink.label);
    let skip = report.turns.len().saturating_sub(RECENT);
    for turn in &report.turns[skip..] {
        let model: String = short(&turn.model).chars().take(19).collect();
        let mut spans = vec![
            Span::styled(format!("{:<6}", turn.at), ink.dim),
            Span::styled(
                format!("{model:<20}"),
                Style::default().fg(colour(&turn.model)),
            ),
            Span::raw(format!(
                "{:>9}{:>9}",
                format_tokens(turn.usage.prompt_tokens()),
                format_tokens(turn.usage.output)
            )),
        ];
        if priced {
            spans.push(Span::styled(
                format!("{:>11}", dollars(turn.usage.cost_micros)),
                Style::default().fg(crate::colour::success()),
            ));
        }
        out.push(Line::from(spans), None);
    }
}

/// What a share is worth writing and measuring: money where anything was priced, tokens otherwise.
fn amount(used: &Usage, priced: bool) -> (f64, String) {
    if priced {
        (
            figure(used.cost_micros),
            format!(
                "{} · {}",
                dollars(used.cost_micros),
                format_tokens(tokens(used))
            ),
        )
    } else {
        (
            figure(tokens(used)),
            format!("{} tok", format_tokens(tokens(used))),
        )
    }
}

/// Rows of what each model took, summed by model, in the order each first appears.
fn gather(rows: impl Iterator<Item = (String, Usage)>) -> Vec<(String, Usage)> {
    let mut out: Vec<(String, Usage)> = Vec::new();
    for (model, used) in rows {
        match out.iter_mut().find(|(known, _)| *known == model) {
            Some((_, total)) => total.add(used),
            None => out.push((model, used)),
        }
    }
    out
}

fn sum(all: impl Iterator<Item = Usage>) -> Usage {
    all.fold(Usage::default(), |mut total, one| {
        total.add(one);
        total
    })
}

fn tokens(used: &Usage) -> u64 {
    used.prompt_tokens() + used.output
}

/// A model's name without whoever routes it: `openrouter/deepseek/v4` reads `deepseek/v4`.
fn short(model: &str) -> &str {
    model
        .split_once('/')
        .filter(|(_, rest)| rest.contains('/'))
        .map_or(model, |(_, rest)| rest)
}

/// The colour of the `nth` model seen.
fn hue(nth: usize) -> Color {
    match nth % 6 {
        0 => crate::colour::accent(),
        1 => crate::colour::code_command(),
        2 => crate::colour::success(),
        3 => crate::colour::warning(),
        4 => crate::colour::said_by_agent(),
        _ => crate::colour::code_operator(),
    }
}

/// A count as a chart coordinate.
fn figure(count: u64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts and micro-dollars, far below where f64 loses precision"
    )]
    let value = count as f64;
    value
}

#[cfg(test)]
#[path = "cost/tallying.rs"]
mod tallying;
