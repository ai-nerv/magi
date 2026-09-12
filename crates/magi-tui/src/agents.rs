//! The run as a tree: every agent under this session's root, drawn indented, for the agents panel.
//! One row per agent; children sit under the one that started them.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One agent in the run, as the panel needs to draw it.
pub struct Agent {
    pub id: String,
    pub role: String,
    /// The id that started it, or `None` for a main — what the tree is built from.
    pub parent: Option<String>,
    /// This viewer's own session.
    pub here: bool,
    /// The session currently drawn on screen.
    pub attached: bool,
    /// In a turn right now, for this many seconds, with this many messages waiting.
    pub busy: bool,
    pub working_for: u64,
    pub waiting: usize,
    /// The work it has claimed, if any.
    pub claim: Option<String>,
}

/// The rows of the agents view: the run's tree, roots first, each child under its parent.
#[must_use]
pub fn lines(agents: &[Agent]) -> Vec<Line<'static>> {
    if agents.is_empty() {
        return Vec::new();
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![
        Line::from(Span::styled(
            format!("{} in this run", counted(agents.len())),
            dim,
        )),
        Line::from(String::new()),
    ];
    for (agent, depth) in tiered(agents) {
        out.push(row(agent, depth));
    }
    out
}

/// `n agents`, or `1 agent`.
fn counted(n: usize) -> String {
    if n == 1 {
        "1 agent".to_owned()
    } else {
        format!("{n} agents")
    }
}

/// One agent's line: indent to depth, a busy/idle dot, `role/id`, then dim live meta — how long it
/// has worked, what waits, what it holds — and a tag for the viewer's own and the one on screen.
fn row(agent: &Agent, depth: usize) -> Line<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    if depth > 0 {
        spans.push(Span::styled(format!("{}└ ", "  ".repeat(depth - 1)), dim));
    }
    let (dot, lit) = if agent.busy {
        ("● ", bold)
    } else {
        ("○ ", dim)
    };
    spans.push(Span::styled(dot.to_owned(), lit));
    spans.push(Span::raw(format!("{}/{}", agent.role, agent.id)));

    let mut meta = String::new();
    if agent.busy && agent.working_for > 0 {
        meta.push_str(&format!("  {}", elapsed(agent.working_for)));
    }
    if agent.waiting > 0 {
        meta.push_str(&format!("  ✉{}", agent.waiting));
    }
    if let Some(claim) = &agent.claim {
        meta.push_str(&format!("  · {claim}"));
    }
    if !meta.is_empty() {
        spans.push(Span::styled(meta, dim));
    }
    if agent.here {
        spans.push(Span::styled("  (you)".to_owned(), bold));
    } else if agent.attached {
        spans.push(Span::styled("  • viewing".to_owned(), dim));
    }
    Line::from(spans)
}

/// A working-for span: `m:ss` once past a minute, else `Ns`.
fn elapsed(secs: u64) -> String {
    if secs >= 60 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// The agents as a tree, depth-first and id-ordered, depth found by walking the parent links within
/// the set. Ring-safe, and anything a note orphaned is listed at the root, so nobody is hidden.
fn tiered(agents: &[Agent]) -> Vec<(&Agent, usize)> {
    use std::collections::BTreeSet;
    let known: BTreeSet<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    let mut roots: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref().is_none_or(|up| !known.contains(up)))
        .collect();
    roots.sort_by(|one, two| one.id.cmp(&two.id));
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for root in roots {
        descend(agents, root, 0, &mut seen, &mut out);
    }
    for agent in agents {
        if seen.insert(agent.id.clone()) {
            out.push((agent, 0));
        }
    }
    out
}

fn descend<'a>(
    agents: &'a [Agent],
    them: &'a Agent,
    depth: usize,
    seen: &mut std::collections::BTreeSet<String>,
    out: &mut Vec<(&'a Agent, usize)>,
) {
    if !seen.insert(them.id.clone()) {
        return;
    }
    out.push((them, depth));
    let mut kids: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref() == Some(them.id.as_str()))
        .collect();
    kids.sort_by(|one, two| one.id.cmp(&two.id));
    for kid in kids {
        descend(agents, kid, depth + 1, seen, out);
    }
}

/// What to say when the run is just this session.
#[must_use]
pub fn empty() -> String {
    "no other agents in this run".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, parent: Option<&str>) -> Agent {
        Agent {
            id: id.to_owned(),
            role: "worker".to_owned(),
            parent: parent.map(ToOwned::to_owned),
            here: false,
            attached: false,
            busy: false,
            working_for: 0,
            waiting: 0,
            claim: None,
        }
    }

    #[test]
    fn a_run_of_three_generations_indents_by_depth() {
        let held = vec![
            agent("phi", Some("theta")),
            agent("alpha", None),
            agent("theta", Some("alpha")),
        ];
        let laid: Vec<(&str, usize)> = tiered(&held)
            .into_iter()
            .map(|(a, d)| (a.id.as_str(), d))
            .collect();
        assert_eq!(laid, vec![("alpha", 0), ("theta", 1), ("phi", 2)]);
    }

    #[test]
    fn an_orphan_whose_parent_is_not_here_is_listed_and_not_hidden() {
        let held = vec![agent("alpha", None), agent("stray", Some("gone"))];
        assert_eq!(tiered(&held).len(), 2);
    }
}
