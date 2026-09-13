//! The run as a tree: every agent under this session's root, drawn with git-log rails, for the
//! agents panel. One row per agent; children sit under the one that started them, and each row is
//! selectable — a click attaches the screen to that agent.

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
    /// What it is doing, as a level — the mechanical phase the harness reports.
    pub phase: magi_proto::Phase,
    /// Why it is `waiting` or `blocked`, when it said.
    pub cause: Option<String>,
    /// How long the current turn has run, and how many messages are waiting for it.
    pub working_for: u64,
    pub waiting: usize,
    /// The work it has claimed, if any.
    pub claim: Option<String>,
}

/// The panel's rows and, parallel to them, the agent each row selects. A header and the blank under
/// it select nothing; every tree row carries the id a click on it attaches to.
pub struct Rendered {
    pub rows: Vec<Line<'static>>,
    pub picks: Vec<Option<String>>,
}

/// The agents view: a count, a blank, then the run's tree drawn with rails, roots first.
#[must_use]
pub fn view(agents: &[Agent]) -> Rendered {
    let mut out = Rendered {
        rows: Vec::new(),
        picks: Vec::new(),
    };
    if agents.is_empty() {
        return out;
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    out.rows.push(Line::from(Span::styled(
        format!("{} in this run", counted(agents.len())),
        dim,
    )));
    out.rows.push(Line::from(String::new()));
    out.picks.push(None);
    out.picks.push(None);
    for (agent, rail) in tiered(agents) {
        out.rows.push(row(agent, &rail));
        out.picks.push(Some(agent.id.clone()));
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

/// One agent's line: the tree rail, a busy/idle dot, `role/id`, then dim live status — working with
/// its timer or idle, what waits, what it holds — and a tag for the viewer's own and the one on screen.
fn row(agent: &Agent, rail: &str) -> Line<'static> {
    use magi_proto::Phase;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    if !rail.is_empty() {
        spans.push(Span::styled(rail.to_owned(), dim));
    }
    // A glyph per phase, lit only while working: the tree should read at a glance, with the one
    // agent doing something now the one that stands out.
    let (dot, lit) = match agent.phase {
        Phase::Working => ("◗ ", bold),
        Phase::Blocked => ("✗ ", bold),
        Phase::Finished => ("✓ ", dim),
        Phase::Waiting => ("⏸ ", dim),
        Phase::Starting => ("◔ ", dim),
        Phase::Gone => ("· ", dim),
        Phase::Idle => ("○ ", dim),
    };
    spans.push(Span::styled(dot.to_owned(), lit));
    spans.push(Span::styled(
        format!("{}/{}", agent.role, agent.id),
        if agent.phase == Phase::Working {
            bold
        } else {
            Style::default()
        },
    ));

    // The phase in words, with the timer on `working` and the cause on `waiting`/`blocked`, so a
    // reader sees not just that an agent is stuck but on what.
    let mut meta = String::from("  ");
    meta.push_str(match agent.phase {
        Phase::Working => "working",
        Phase::Blocked => "blocked",
        Phase::Finished => "finished",
        Phase::Waiting => "waiting",
        Phase::Starting => "starting",
        Phase::Gone => "gone",
        Phase::Idle => "idle",
    });
    if agent.phase == Phase::Working && agent.working_for > 0 {
        meta.push_str(&format!(" {}", elapsed(agent.working_for)));
    }
    if matches!(agent.phase, Phase::Waiting | Phase::Blocked)
        && let Some(cause) = &agent.cause
    {
        meta.push_str(&format!(": {cause}"));
    }
    if agent.waiting > 0 {
        meta.push_str(&format!("  ✉{}", agent.waiting));
    }
    if let Some(claim) = &agent.claim {
        meta.push_str(&format!("  · {claim}"));
    }
    spans.push(Span::styled(meta, dim));

    if agent.here {
        spans.push(Span::styled("  (you)".to_owned(), bold));
    } else if agent.attached {
        spans.push(Span::styled(
            "  • viewing".to_owned(),
            Style::default()
                .fg(crate::colour::hint())
                .add_modifier(Modifier::BOLD),
        ));
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

/// The agents as a tree, depth-first and id-ordered, each paired with the rail drawn to its left.
/// Ring-safe, and anything a note orphaned is listed at the root, so nobody is hidden.
fn tiered(agents: &[Agent]) -> Vec<(&Agent, String)> {
    use std::collections::BTreeSet;
    let known: BTreeSet<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    let mut roots: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref().is_none_or(|up| !known.contains(up)))
        .collect();
    roots.sort_by(|one, two| one.id.cmp(&two.id));
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for root in &roots {
        descend(agents, root, "", true, 0, &mut seen, &mut out);
    }
    // Anything a ring hid from the walk is still owed a row, listed flat at the root.
    for agent in agents {
        if seen.insert(agent.id.clone()) {
            out.push((agent, String::new()));
        }
    }
    out
}

/// Walk one node and its children, building the rail as we go: `├─ ` or `└─ ` for the node, and
/// `│  ` or three spaces carried down for each ancestor depending on whether it had more below it.
fn descend<'a>(
    agents: &'a [Agent],
    them: &'a Agent,
    prefix: &str,
    last: bool,
    depth: usize,
    seen: &mut std::collections::BTreeSet<String>,
    out: &mut Vec<(&'a Agent, String)>,
) {
    if !seen.insert(them.id.clone()) {
        return;
    }
    let rail = if depth == 0 {
        String::new()
    } else {
        format!("{prefix}{}", if last { "└─ " } else { "├─ " })
    };
    out.push((them, rail));
    let child_prefix = if depth == 0 {
        String::new()
    } else {
        format!("{prefix}{}", if last { "   " } else { "│  " })
    };
    let mut kids: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref() == Some(them.id.as_str()))
        .collect();
    kids.sort_by(|one, two| one.id.cmp(&two.id));
    let end = kids.len().saturating_sub(1);
    for (at, kid) in kids.into_iter().enumerate() {
        descend(agents, kid, &child_prefix, at == end, depth + 1, seen, out);
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
            phase: magi_proto::Phase::Idle,
            cause: None,
            working_for: 0,
            waiting: 0,
            claim: None,
        }
    }

    /// The ids in draw order, each with the depth its rail implies (two rail cells per level).
    fn laid(held: &[Agent]) -> Vec<(String, usize)> {
        tiered(held)
            .into_iter()
            .map(|(a, rail)| (a.id.clone(), rail.chars().count() / 3))
            .collect()
    }

    #[test]
    fn a_run_of_three_generations_indents_by_depth() {
        let held = vec![
            agent("phi", Some("theta")),
            agent("alpha", None),
            agent("theta", Some("alpha")),
        ];
        assert_eq!(
            laid(&held),
            vec![
                ("alpha".to_owned(), 0),
                ("theta".to_owned(), 1),
                ("phi".to_owned(), 2)
            ]
        );
    }

    #[test]
    fn the_last_child_gets_the_corner_and_the_rest_a_tee() {
        let held = vec![
            agent("root", None),
            agent("a", Some("root")),
            agent("b", Some("root")),
        ];
        let rails: Vec<String> = tiered(&held).into_iter().map(|(_, rail)| rail).collect();
        assert_eq!(rails[0], "");
        assert!(rails[1].starts_with("├─"), "{rails:?}");
        assert!(rails[2].starts_with("└─"), "{rails:?}");
    }

    #[test]
    fn an_orphan_whose_parent_is_not_here_is_listed_and_not_hidden() {
        let held = vec![agent("alpha", None), agent("stray", Some("gone"))];
        assert_eq!(tiered(&held).len(), 2);
    }

    #[test]
    fn a_row_is_offered_for_every_agent_and_none_for_the_header() {
        let held = vec![agent("alpha", None), agent("beta", Some("alpha"))];
        let rendered = view(&held);
        assert_eq!(rendered.rows.len(), rendered.picks.len());
        assert_eq!(rendered.picks[0], None, "the count line selects nothing");
        assert_eq!(rendered.picks[1], None, "nor the blank under it");
        let picked: Vec<&str> = rendered
            .picks
            .iter()
            .flatten()
            .map(String::as_str)
            .collect();
        assert_eq!(picked, vec!["alpha", "beta"]);
    }

    /// The whole point of the phase: `finished` reads differently from `idle`, and a coordinator
    /// can see it at a glance.
    #[test]
    fn the_phase_shows_in_the_row() {
        let text = |a: &Agent| -> String {
            row(a, "")
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        let mut a = agent("psi", None);
        a.phase = magi_proto::Phase::Working;
        a.working_for = 5;
        assert!(
            text(&a).contains("◗") && text(&a).contains("working 5s"),
            "{}",
            text(&a)
        );
        a.phase = magi_proto::Phase::Finished;
        assert!(
            text(&a).contains("✓") && text(&a).contains("finished"),
            "{}",
            text(&a)
        );
        a.phase = magi_proto::Phase::Blocked;
        a.cause = Some("run declined".to_owned());
        assert!(text(&a).contains("blocked: run declined"), "{}", text(&a));
        a.phase = magi_proto::Phase::Idle;
        assert!(
            text(&a).contains("○") && text(&a).contains("idle"),
            "{}",
            text(&a)
        );
    }
}
