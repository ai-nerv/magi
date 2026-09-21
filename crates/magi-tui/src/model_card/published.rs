//! The card's published half: what a million tokens costs, what the model is and can do, how it
//! scores, and who serves it at what price — the one part of it that can be chosen from.

use super::{Card, Details, Endpoint, Ink, Item, Known, Rendered, charts};
use crate::footer::format_tokens;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// How many providers are listed, cheapest first; the rest are counted.
const LISTED: usize = 12;

/// The request parameters worth naming, and what to call each.
const FEATURES: [(&str, &str); 9] = [
    ("tools", "tools"),
    ("reasoning", "reasoning"),
    ("structured_outputs", "structured output"),
    ("response_format", "json mode"),
    ("tool_choice", "tool choice"),
    ("parallel_tool_calls", "parallel tools"),
    ("web_search_options", "web search"),
    ("logprobs", "logprobs"),
    ("seed", "seed"),
];

/// Every published section, or why there are none.
pub(super) fn sections(out: &mut Rendered, card: &Card<'_>, ink: &Ink, width: u16) {
    let details = match &card.details {
        Known::Asking => {
            out.section("Published", "", width);
            out.say("asking the provider about it…", ink.dim);
            return;
        }
        Known::Missing(why) => {
            out.section("Published", "", width);
            out.say(*why, ink.dim);
            return;
        }
        Known::Found(details) => *details,
    };
    price(out, details, width);
    facts(out, details, ink, width);
    scores(out, details, width);
    providers(out, details, card.provider, ink, width);
}

/// What a million tokens costs each way, as bars against the dearest.
fn price(out: &mut Rendered, details: &Details, width: u16) {
    let [input, output, read, write] = details.price;
    let items: Vec<Item> = [
        ("input", input, crate::colour::code_command()),
        ("output", output, crate::colour::accent()),
        ("cache read", read, crate::colour::success()),
        ("cache write", write, crate::colour::warning()),
    ]
    .into_iter()
    .filter(|(_, price, _)| *price > 0.0)
    .map(|(label, price, ink)| Item {
        label: label.to_owned(),
        value: price,
        said: money(price),
        ink,
    })
    .collect();
    if items.is_empty() {
        return;
    }
    out.section("Price", "per million tokens", width);
    let top = items.iter().map(|item| item.value).fold(0.0_f64, f64::max);
    out.chart(charts::bars(&items, top, width));
}

/// What the model is: how much it writes, what it knows, what it takes, when it came out, and what
/// a request may ask of it.
fn facts(out: &mut Rendered, details: &Details, ink: &Ink, width: u16) {
    out.section("Model", "", width);
    if let Some(ceiling) = details.max_output {
        out.fact(
            "Output",
            format!("up to {} tokens", format_tokens(ceiling)),
            ink,
        );
    }
    if let Some(cutoff) = &details.knowledge_cutoff {
        out.fact("Knows up to", cutoff.clone(), ink);
    }
    let takes = if details.inputs.is_empty() || details.outputs.is_empty() {
        details
            .modality
            .clone()
            .map(|said| said.replace("->", " → "))
    } else {
        Some(format!(
            "{} → {}",
            details.inputs.join(", "),
            details.outputs.join(", ")
        ))
    };
    if let Some(takes) = takes {
        out.fact("Takes", takes, ink);
    }
    if let Some(tokenizer) = &details.tokenizer {
        out.fact("Tokenizer", tokenizer.clone(), ink);
    }
    if let Some(created) = details.created {
        out.fact("Released", date(created), ink);
    }
    if let Some(moderated) = details.moderated {
        out.fact("Moderated", if moderated { "yes" } else { "no" }, ink);
    }
    let can: Vec<&str> = FEATURES
        .iter()
        .filter(|(key, _)| details.features.iter().any(|has| has == key))
        .map(|(_, name)| *name)
        .collect();
    let room = usize::from(width).saturating_sub(14);
    let mut label = "Can";
    let mut row = String::new();
    for name in can {
        if !row.is_empty() && row.chars().count() + name.chars().count() + 3 > room {
            chips(out, label, &row, ink);
            label = "";
            row.clear();
        }
        if !row.is_empty() {
            row.push_str(" · ");
        }
        row.push_str(name);
    }
    if !row.is_empty() {
        chips(out, label, &row, ink);
    }
}

/// One row of what a request may ask for, in the success colour.
fn chips(out: &mut Rendered, label: &str, row: &str, ink: &Ink) {
    out.push(
        Line::from(vec![
            Span::styled(format!("{label:<14}"), ink.label),
            Span::styled(
                row.to_owned(),
                Style::default().fg(crate::colour::success()),
            ),
        ]),
        None,
    );
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
            value: *score,
            said: format!("{score:.1}"),
            ink: *ink,
        })
        .collect();
    out.section("Benchmarks", "Artificial Analysis, out of 100", width);
    out.chart(charts::bars(&items, 100.0, width));
}

/// Who serves it, cheapest first, each a row that can be taken: the router's own choice first, then
/// each provider with its price, window and speed.
fn providers(out: &mut Rendered, details: &Details, chosen: Option<&str>, ink: &Ink, width: u16) {
    let mut serving: Vec<&Endpoint> = details
        .endpoints
        .iter()
        .filter(|endpoint| endpoint.tag.is_some())
        .collect();
    if serving.is_empty() {
        return;
    }
    serving.sort_by(|a, b| cost(a).total_cmp(&cost(b)));
    let note = format!("⏎ to choose · {} serve it", serving.len());
    out.section("Providers", &note, width);
    let marker = |on: bool| {
        if on {
            Span::styled("◉ ", ink.value)
        } else {
            Span::styled("○ ", ink.dim)
        }
    };
    out.push(
        Line::from(vec![
            marker(chosen.is_none()),
            Span::styled(
                format!("{:<17}", "auto"),
                Style::default().fg(crate::colour::text()),
            ),
            Span::styled("the router picks, and falls back", ink.dim),
        ]),
        Some("provider:"),
    );
    for endpoint in serving.iter().take(LISTED) {
        let tag = endpoint.tag.as_deref().unwrap_or_default();
        let name: String = endpoint.provider.chars().take(16).collect();
        let mut spans = vec![
            marker(chosen == Some(tag)),
            Span::styled(
                format!("{name:<17}"),
                Style::default().fg(crate::colour::text()),
            ),
            Span::styled(
                format!("{:<6}", endpoint.quantization.as_deref().unwrap_or("")),
                ink.dim,
            ),
            Span::styled(
                format!(
                    "{:<16}",
                    format!(
                        "{} / {}",
                        money(endpoint.price[0]),
                        money(endpoint.price[1])
                    )
                ),
                Style::default().fg(priced(endpoint)),
            ),
        ];
        if let Some(window) = endpoint.context {
            spans.push(Span::styled(
                format!("{:<7}", format_tokens(window)),
                ink.dim,
            ));
        }
        if let Some(speed) = endpoint.throughput {
            spans.push(Span::styled(format!("{speed:.0} t/s"), ink.dim));
        } else if let Some(wait) = endpoint.latency_ms {
            spans.push(Span::styled(format!("{:.1} s", wait / 1000.0), ink.dim));
        }
        let pick = format!("provider:{tag}");
        out.push(Line::from(spans), Some(&pick));
    }
    if serving.len() > LISTED {
        out.say(
            format!("  … {} dearer ones not listed", serving.len() - LISTED),
            ink.dim,
        );
    }
}

/// What a provider charges for a million in and a million out, for ordering; unpriced sorts last.
fn cost(endpoint: &Endpoint) -> f64 {
    let each = endpoint.price[0] + endpoint.price[1];
    if each > 0.0 { each } else { f64::MAX }
}

/// Free in green, anything else in the price colour.
fn priced(endpoint: &Endpoint) -> Color {
    if cost(endpoint) == f64::MAX {
        crate::colour::dim()
    } else {
        crate::colour::success()
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

/// Seconds since the epoch as a calendar date, `YYYY-MM-DD`. Howard Hinnant's civil-from-days, so
/// no clock crate is taken on for one line of a card.
pub(crate) fn date(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
