//! What the last request was built from, as balthasar laid it out: how the window was shared, what
//! went whole, stubbed or left out, and why. Drawn the way the model's card is, a section each.

use crate::footer::format_tokens;
use crate::model_card::Rendered;
use crate::model_card::charts::{self, Item};
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};

/// One layout, as the session reported it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Laid {
    /// balthasar's name for it; empty when magi made it up with nobody to ask.
    pub id: String,
    pub budget: serde_json::Value,
    pub counts: magi_proto::Laid,
    pub why: String,
    pub slots: Vec<magi_proto::laying::LaidSlot>,
}

impl Laid {
    /// How the request divided by what each part cost: `conversation 62% · summary 8% · fixed 11k`.
    #[must_use]
    pub fn split(&self) -> Option<String> {
        let sum = |kinds: &[&str]| -> u64 {
            self.slots
                .iter()
                .filter(|slot| kinds.contains(&slot.kind.as_str()))
                .map(|slot| slot.tokens)
                .sum()
        };
        let parts = [
            ("conversation", sum(&["item", "stub"])),
            ("summary", sum(&["summary"])),
            ("memory", sum(&["memory"])),
            ("notes", sum(&["pinned", "rules", "observations", "note"])),
        ];
        let fixed = self.number("fixed");
        let total = parts.iter().map(|(_, n)| n).sum::<u64>() + fixed;
        if self.slots.is_empty() || total == 0 {
            return None;
        }
        let mut out: Vec<String> = parts
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(name, n)| format!("{name} {}%", n * 100 / total))
            .collect();
        if fixed > 0 {
            out.push(format!("fixed {}", format_tokens(fixed)));
        }
        Some(out.join(" · "))
    }

    /// What the request held, in a line.
    #[must_use]
    pub fn composition(&self) -> String {
        let counted = &self.counts;
        let mut parts = vec![format!("{} whole", counted.items)];
        if counted.stubs > 0 {
            parts.push(format!("{} stubbed", counted.stubs));
        }
        if counted.dropped > 0 {
            parts.push(format!("{} left out", counted.dropped));
        }
        for (count, what) in [
            (counted.summary, "summary"),
            (counted.memory, "memory"),
            (counted.pinned, "pinned"),
            (counted.notes, "notes"),
        ] {
            if count > 0 {
                parts.push(what.to_owned());
            }
        }
        parts.join(" · ")
    }

    fn number(&self, key: &str) -> u64 {
        self.budget
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    }
}

/// What to say before anything has been laid out.
#[must_use]
pub fn empty() -> String {
    "nothing laid out yet — this fills in with the next request".to_owned()
}

/// The whole view, top to bottom, with the helper jobs this session has run beneath it.
#[must_use]
pub fn view(laid: &Laid, jobs: &[crate::cost::Helper], width: u16) -> Rendered {
    let ink = crate::model_card::ink();
    let width = width.max(20);
    let mut out = Rendered::default();
    out.say(laid.composition(), ink.value.add_modifier(Modifier::BOLD));
    out.say(
        if laid.id.is_empty() {
            "laid out by magi — balthasar was not asked or did not answer".to_owned()
        } else {
            format!("layout {} from balthasar", laid.id)
        },
        ink.dim,
    );
    if let Some(split) = laid.split() {
        out.say(split, ink.dim);
    }
    budget(&mut out, laid, width);

    if !laid.slots.is_empty() {
        out.section("Slots", "in the order they were sent", width);
        for slot in &laid.slots {
            let at = slot.cursor.map(|c| c.to_string()).unwrap_or_default();
            out.push(
                Line::from(vec![
                    Span::styled(format!("{:<9}", slot.kind), ink.label),
                    Span::styled(format!("{at:>6} "), ink.dim),
                    Span::raw(format!("{:>6}  ", format_tokens(slot.tokens))),
                    Span::raw(slot.text.clone()),
                ]),
                None,
            );
        }
    }

    out.section("Sent", "what went into the last request", width);
    let counted = laid.counts;
    out.fact("Whole", counted.items.to_string(), &ink);
    out.fact("Stubbed", counted.stubs.to_string(), &ink);
    out.fact("Left out", counted.dropped.to_string(), &ink);
    let written = [
        (counted.summary, "Summary"),
        (counted.memory, "Memory"),
        (counted.pinned, "Pinned"),
        (counted.notes, "Notes"),
    ];
    for (count, label) in written {
        if count > 0 {
            out.fact(label, count.to_string(), &ink);
        }
    }

    if !laid.why.is_empty() {
        out.section("Why", "", width);
        for line in crate::wrap::line(Line::from(laid.why.clone()), width) {
            out.say(line.to_string(), ink.dim);
        }
    }

    if !jobs.is_empty() {
        out.section("Helper jobs", "the newest last", width);
        for job in jobs.iter().rev().take(JOBS).rev() {
            let used = job.usage;
            out.fact(
                &job.role,
                format!(
                    "{} · {} in, {} out · {}",
                    job.model,
                    format_tokens(used.prompt_tokens()),
                    format_tokens(used.output),
                    crate::cost::dollars(used.cost_micros)
                ),
                &ink,
            );
        }
    }
    out
}

/// How many helper jobs the view lists.
const JOBS: usize = 8;

/// How the window was shared out, each part a bar against the whole of it.
fn budget(out: &mut Rendered, laid: &Laid, width: u16) {
    let window = laid.number("window");
    if window == 0 {
        return;
    }
    let note = format!("of a {} window", format_tokens(window));
    out.section("Budget", &note, width);
    let parts: [(&str, &str, Color); 6] = [
        ("fixed", "system, tools", crate::colour::code_command()),
        ("reply", "the answer", crate::colour::warning()),
        ("conversation", "conversation", crate::colour::accent()),
        ("summary", "summary", crate::colour::success()),
        ("memory", "memory", crate::colour::said_by_agent()),
        ("pinned", "project notes", crate::colour::code_operator()),
    ];
    let items: Vec<Item> = parts
        .iter()
        .filter_map(|(key, label, ink)| {
            let value = laid.number(key);
            (value > 0).then(|| Item {
                label: (*label).to_owned(),
                value: figure(value),
                said: format_tokens(value),
                ink: *ink,
            })
        })
        .collect();
    out.chart(charts::bars(&items, figure(window), width));

    let sent = laid.number("estimated_input").max(laid.number("used"));
    if sent > 0 {
        let share = (figure(sent) / figure(window)).min(1.0);
        let ink = match share {
            s if s > 0.9 => crate::colour::error(),
            s if s > 0.7 => crate::colour::warning(),
            _ => crate::colour::success(),
        };
        out.blank();
        let label = format!("{} of {} sent", format_tokens(sent), format_tokens(window));
        out.chart(charts::gauge(share, &label, ink, width));
    }
    if let Some(factor) = laid
        .budget
        .get("factor")
        .and_then(serde_json::Value::as_f64)
    {
        out.blank();
        out.fact(
            "Correction",
            format!("×{factor:.2} — what the provider counted over the estimate"),
            &crate::model_card::ink(),
        );
    }
}

/// A count as a chart coordinate.
fn figure(count: u64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts, far below where f64 loses precision"
    )]
    let value = count as f64;
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laid() -> Laid {
        Laid {
            id: "L-3".into(),
            budget: serde_json::json!({
                "window": 200_000, "fixed": 11_000, "reply": 32_000,
                "conversation": 120_000, "memory": 4_000, "estimated_input": 70_000, "factor": 1.08,
            }),
            counts: magi_proto::Laid {
                items: 40,
                stubs: 3,
                dropped: 12,
                summary: 1,
                memory: 1,
                ..magi_proto::Laid::default()
            },
            why: "the conversation is long".into(),
            slots: vec![
                slot("summary", None, 2_000, "they read four files"),
                slot("stub", Some(5), 12, "read part1.txt (3000 lines)"),
                slot("item", Some(9), 6_000, "you: now summarise them"),
            ],
        }
    }

    fn slot(
        kind: &str,
        cursor: Option<u64>,
        tokens: u64,
        text: &str,
    ) -> magi_proto::laying::LaidSlot {
        magi_proto::laying::LaidSlot {
            kind: kind.into(),
            cursor,
            tokens,
            text: text.into(),
        }
    }

    #[test]
    fn split_counts_verified_rules_and_observations_as_project_notes() {
        let view = Laid {
            slots: vec![
                slot("rules", None, 20, "Rule"),
                slot("observations", None, 30, "Observation"),
                slot("item", Some(1), 50, "User"),
            ],
            ..Laid::default()
        };
        assert_eq!(
            view.split().as_deref(),
            Some("conversation 50% · notes 50%")
        );
    }

    #[test]
    fn the_split_is_what_each_part_cost_against_the_whole() {
        // 6,012 conversation, 2,000 summary and 11,000 fixed: 19,012 in all.
        assert_eq!(
            laid().split().as_deref(),
            Some("conversation 31% · summary 10% · fixed 11k")
        );
    }

    #[test]
    fn every_slot_is_listed_with_its_cost() {
        let all: String = view(&laid(), &[], 80)
            .rows
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("read part1.txt (3000 lines)"), "{all}");
        assert!(all.contains("they read four files"), "{all}");
    }

    #[test]
    fn the_line_says_what_went_and_what_did_not() {
        assert_eq!(
            laid().composition(),
            "40 whole · 3 stubbed · 12 left out · summary · memory"
        );
    }

    #[test]
    fn the_view_shows_the_budget_the_counts_and_the_reason() {
        let jobs = [crate::cost::Helper {
            role: "memory".into(),
            model: "p/small".into(),
            usage: magi_proto::Usage::default(),
        }];
        let drawn: Vec<String> = view(&laid(), &jobs, 60)
            .rows
            .iter()
            .map(ToString::to_string)
            .collect();
        let all = drawn.join("\n");
        for wanted in [
            "Budget",
            "L-3",
            "Left out",
            "×1.08",
            "the conversation is long",
            "Helper jobs",
            "p/small",
        ] {
            assert!(all.contains(wanted), "{wanted} is missing:\n{all}");
        }
    }

    #[test]
    fn a_layout_magi_made_itself_says_so() {
        let mine = Laid {
            id: String::new(),
            ..laid()
        };
        let all: String = view(&mine, &[], 60)
            .rows
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(all.contains("laid out by magi"), "{all}");
    }
}
