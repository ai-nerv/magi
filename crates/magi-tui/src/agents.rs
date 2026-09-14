//! The run as a tree: every agent under this session's root, drawn with git-log rails, for the
//! agents panel. Three rows per agent — who it is, what it is doing, what it is for — and children
//! under the one that started them. Every row of an entry selects it; attaching goes to that agent.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;

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
    /// What its role is for, when the configuration says.
    pub about: Option<String>,
}

/// The panel's rows and, parallel to them, the agent each row selects. A header and the blank under
/// it select nothing; every row of an entry carries the id attaching to it goes to.
pub struct Rendered {
    pub rows: Vec<Line<'static>>,
    pub picks: Vec<Option<String>>,
}

/// The agents view: a count, a blank, then the run's tree, roots first. Anything under an id in
/// `folded` is left out, and the folded agent says how many.
#[must_use]
pub fn view(agents: &[Agent], folded: &BTreeSet<String>) -> Rendered {
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
    for placed in tiered(agents, folded) {
        for line in entry(&placed) {
            out.rows.push(line);
            out.picks.push(Some(placed.agent.id.clone()));
        }
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

/// Where one agent lands in the tree: the rail beside its name, the rail beside the two rows under
/// the name, and how many agents below it are folded away.
struct Placed<'a> {
    agent: &'a Agent,
    head: String,
    body: String,
    hidden: usize,
}

/// One agent's three rows: a phase glyph and `role/id` with its tags, then what it is doing, then
/// what it is for — the role's description, or who started it.
fn entry(placed: &Placed<'_>) -> [Line<'static>; 3] {
    use magi_proto::Phase;
    let agent = placed.agent;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let rail = |text: &str| Span::styled(text.to_owned(), dim);

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
    let mut name = vec![
        rail(&placed.head),
        Span::styled(dot.to_owned(), lit),
        Span::styled(agent.role.clone(), bold),
        Span::raw(format!("/{}", agent.id)),
    ];
    if agent.here {
        name.push(Span::styled("  (you)".to_owned(), bold));
    } else if agent.attached {
        name.push(Span::styled(
            "  • viewing".to_owned(),
            Style::default()
                .fg(crate::colour::hint())
                .add_modifier(Modifier::BOLD),
        ));
    }
    if placed.hidden > 0 {
        name.push(Span::styled(format!("  ▸ {} folded", placed.hidden), dim));
    }

    let about = agent
        .about
        .clone()
        .or_else(|| agent.parent.as_ref().map(|up| format!("started by {up}")))
        .unwrap_or_else(|| "the root of this run".to_owned());
    [
        Line::from(name),
        Line::from(vec![rail(&placed.body), Span::styled(doing(agent), dim)]),
        Line::from(vec![
            rail(&placed.body),
            Span::styled(about, dim.add_modifier(Modifier::ITALIC)),
        ]),
    ]
}

/// The phase in words, with the timer on `working` and the cause on `waiting`/`blocked`, so a
/// reader sees not just that an agent is stuck but on what; then what waits for it and what it holds.
fn doing(agent: &Agent) -> String {
    use magi_proto::Phase;
    let mut said = String::from(match agent.phase {
        Phase::Working => "working",
        Phase::Blocked => "blocked",
        Phase::Finished => "finished",
        Phase::Waiting => "waiting",
        Phase::Starting => "starting",
        Phase::Gone => "gone",
        Phase::Idle => "idle",
    });
    if agent.phase == Phase::Working && agent.working_for > 0 {
        said.push_str(&format!(" {}", elapsed(agent.working_for)));
    }
    if matches!(agent.phase, Phase::Waiting | Phase::Blocked)
        && let Some(cause) = &agent.cause
    {
        said.push_str(&format!(": {cause}"));
    }
    if agent.waiting > 0 {
        said.push_str(&format!("  ✉{}", agent.waiting));
    }
    if let Some(claim) = &agent.claim {
        said.push_str(&format!("  · {claim}"));
    }
    said
}

/// A working-for span: `m:ss` once past a minute, else `Ns`.
fn elapsed(secs: u64) -> String {
    if secs >= 60 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// The agents as a tree, depth-first and id-ordered, each with its rails. Ring-safe, and anything
/// a note orphaned is listed at the root, so nobody is hidden who was not folded away.
fn tiered<'a>(agents: &'a [Agent], folded: &'a BTreeSet<String>) -> Vec<Placed<'a>> {
    let known: BTreeSet<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    let mut roots: Vec<&Agent> = agents
        .iter()
        .filter(|a| a.parent.as_deref().is_none_or(|up| !known.contains(up)))
        .collect();
    roots.sort_by(|one, two| one.id.cmp(&two.id));
    let mut walk = Walk {
        agents,
        folded,
        seen: BTreeSet::new(),
        out: Vec::new(),
    };
    for root in roots {
        walk.descend(root, "", true, 0);
    }
    // Anything a ring hid from the walk is still owed an entry, listed flat at the root.
    for agent in agents {
        if walk.seen.insert(agent.id.clone()) {
            walk.out.push(Placed {
                agent,
                head: String::new(),
                body: "  ".to_owned(),
                hidden: 0,
            });
        }
    }
    walk.out
}

/// One walk of the tree: what has been drawn, and what is shut.
struct Walk<'a> {
    agents: &'a [Agent],
    folded: &'a BTreeSet<String>,
    seen: BTreeSet<String>,
    out: Vec<Placed<'a>>,
}

impl<'a> Walk<'a> {
    /// The agents `id` started, by id.
    fn kids(&self, id: &str) -> Vec<&'a Agent> {
        let agents = self.agents;
        let mut kids: Vec<&'a Agent> = agents
            .iter()
            .filter(|a| a.parent.as_deref() == Some(id))
            .collect();
        kids.sort_by(|one, two| one.id.cmp(&two.id));
        kids
    }

    /// Place one node and its children. `├─ ` or `└─ ` beside the name, and carried down for each
    /// ancestor `│  ` or three spaces, depending on whether it had more below it. The rows under a
    /// name carry a `│` under its glyph when children hang below, so the rail does not break.
    fn descend(&mut self, them: &'a Agent, prefix: &str, last: bool, depth: usize) {
        if !self.seen.insert(them.id.clone()) {
            return;
        }
        let (head, carry) = if depth == 0 {
            (String::new(), String::new())
        } else {
            (
                format!("{prefix}{}", if last { "└─ " } else { "├─ " }),
                format!("{prefix}{}", if last { "   " } else { "│  " }),
            )
        };
        let kids = self.kids(&them.id);
        let shut = !kids.is_empty() && self.folded.contains(&them.id);
        let hidden = if shut { self.hide(&them.id) } else { 0 };
        let body = format!(
            "{carry}{}",
            if kids.is_empty() || shut {
                "  "
            } else {
                "│ "
            }
        );
        self.out.push(Placed {
            agent: them,
            head,
            body,
            hidden,
        });
        if shut {
            return;
        }
        let end = kids.len().saturating_sub(1);
        for (at, kid) in kids.into_iter().enumerate() {
            self.descend(kid, &carry, at == end, depth + 1);
        }
    }

    /// Count everything under `id` as placed without placing it, and say how many that was.
    fn hide(&mut self, id: &str) -> usize {
        let mut count = 0;
        for kid in self.kids(id) {
            if self.seen.insert(kid.id.clone()) {
                count += 1 + self.hide(&kid.id);
            }
        }
        count
    }
}

/// What to say when the run is just this session.
#[must_use]
pub fn empty() -> String {
    "no other agents in this run".to_owned()
}

#[cfg(test)]
#[path = "agents/tests.rs"]
mod tests;
