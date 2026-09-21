//! The project's notes as the memory layer keeps them, and the log of every change to them: where a
//! person reads what the agent has learnt, undoes a change, or approves or rejects a staged one.

use crate::model_card::Rendered;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

/// What to say before the memory layer has answered.
#[must_use]
pub fn empty() -> String {
    "asking the memory layer for its notes…".to_owned()
}

fn field<'a>(row: &'a serde_json::Value, key: &str) -> &'a str {
    row.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

fn rows<'a>(notes: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    notes
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn first_line(text: &str, width: usize) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(width)
        .collect()
}

/// The whole view: the pinned notes, the rest by id, and the change log with what can be done to it.
#[must_use]
pub fn view(
    notes: Option<&serde_json::Value>,
    changes: &[serde_json::Value],
    width: u16,
) -> Rendered {
    let ink = crate::model_card::ink();
    let width = width.max(20);
    let text_width = usize::from(width).saturating_sub(16);
    let mut out = Rendered::default();
    let Some(notes) = notes else {
        return out;
    };
    let (pinned, deferred) = (rows(notes, "pinned"), rows(notes, "deferred"));
    out.say(
        format!(
            "{} pinned · {} more by id · {} changes",
            pinned.len(),
            deferred.len(),
            changes.len()
        ),
        ink.value.add_modifier(Modifier::BOLD),
    );
    out.say(
        "pinned notes go into every request; the rest by title, until the model opens one",
        ink.dim,
    );
    if !pinned.is_empty() {
        out.section("Pinned", "in every request", width);
        for note in pinned {
            let title = first_line(field(note, "title"), 13);
            out.fact(&title, first_line(field(note, "text"), text_width), &ink);
        }
    }
    if !deferred.is_empty() {
        out.section("By id", "the model opens one with its note tool", width);
        for note in deferred {
            let title = first_line(field(note, "title"), 13);
            let said = format!("{} — {}", field(note, "id"), field(note, "description"));
            out.fact(&title, first_line(&said, text_width), &ink);
        }
    }
    out.section(
        "Changes",
        "newest first · ⏎ undoes an applied one, approves or rejects a staged one",
        width,
    );
    for change in changes {
        let id = field(change, "id");
        let state = field(change, "state");
        let what = [&change["after"]["title"], &change["before"]["title"]]
            .into_iter()
            .find_map(serde_json::Value::as_str)
            .unwrap_or_else(|| field(change, "note"));
        let line = Line::from(vec![
            Span::styled(format!("{state:<9}"), ink.label),
            Span::raw(first_line(
                &format!("{} {what}", field(change, "op")),
                text_width,
            )),
            Span::styled(format!("  by {}", field(change, "by")), ink.dim),
        ]);
        match state {
            "applied" => out.push(line, Some(&format!("undo:{id}"))),
            "staged" => {
                out.push(line, Some(&format!("approve:{id}")));
                out.push(
                    Line::from(Span::styled(
                        "         reject it instead".to_owned(),
                        ink.dim,
                    )),
                    Some(&format!("reject:{id}")),
                );
            }
            _ => out.push(line, None),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_changes_are_listed_and_only_what_can_be_done_is_a_choice() {
        let notes = serde_json::json!({
            "pinned": [{ "id": "n1", "title": "tooling", "text": "use uv, not pip" }],
            "deferred": [{ "id": "n2", "title": "deploy", "description": "how releases go out" }],
        });
        let changes = [
            serde_json::json!({ "id": "c2", "op": "add", "state": "staged", "by": "extract",
                                "after": { "title": "deploy" } }),
            serde_json::json!({ "id": "c1", "op": "add", "state": "applied", "by": "extract",
                                "after": { "title": "tooling" } }),
            serde_json::json!({ "id": "c0", "op": "update", "state": "undone", "by": "extract" }),
        ];
        let drawn = view(Some(&notes), &changes, 80);
        let all: Vec<String> = drawn.rows.iter().map(ToString::to_string).collect();
        let all = all.join("\n");
        for wanted in [
            "use uv, not pip",
            "n2 — how releases go out",
            "add deploy",
            "by extract",
        ] {
            assert!(all.contains(wanted), "{wanted} is missing:\n{all}");
        }
        let picks: Vec<&str> = drawn.picks.iter().flatten().map(String::as_str).collect();
        assert_eq!(picks, ["approve:c2", "reject:c2", "undo:c1"]);
    }

    #[test]
    fn nothing_is_drawn_before_the_answer() {
        assert!(view(None, &[], 80).rows.is_empty());
    }
}
