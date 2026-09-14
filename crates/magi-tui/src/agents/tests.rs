//! The tree, its entries, and what folding leaves out.
//!
//! Split out under THE RULE; the view these draw is next door.

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
        about: None,
    }
}

fn open() -> BTreeSet<String> {
    BTreeSet::new()
}

/// The ids in draw order, each with the depth its rail implies (three rail cells per level).
fn laid(held: &[Agent], folded: &BTreeSet<String>) -> Vec<(String, usize)> {
    tiered(held, folded)
        .into_iter()
        .map(|placed| (placed.agent.id.clone(), placed.head.chars().count() / 3))
        .collect()
}

fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn a_run_of_three_generations_indents_by_depth() {
    let held = vec![
        agent("phi", Some("theta")),
        agent("alpha", None),
        agent("theta", Some("alpha")),
    ];
    assert_eq!(
        laid(&held, &open()),
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
    let folded = open();
    let rails: Vec<String> = tiered(&held, &folded)
        .into_iter()
        .map(|placed| placed.head)
        .collect();
    assert_eq!(rails[0], "");
    assert!(rails[1].starts_with("├─"), "{rails:?}");
    assert!(rails[2].starts_with("└─"), "{rails:?}");
}

#[test]
fn an_orphan_whose_parent_is_not_here_is_listed_and_not_hidden() {
    let held = vec![agent("alpha", None), agent("stray", Some("gone"))];
    assert_eq!(tiered(&held, &open()).len(), 2);
}

#[test]
fn every_agent_is_three_rows_and_every_row_selects_it() {
    let held = vec![agent("alpha", None), agent("beta", Some("alpha"))];
    let rendered = view(&held, &open());
    assert_eq!(rendered.rows.len(), rendered.picks.len());
    assert_eq!(
        rendered.rows.len(),
        2 + 2 * 3,
        "the count, a blank, two entries"
    );
    assert_eq!(rendered.picks[0], None, "the count line selects nothing");
    assert_eq!(rendered.picks[1], None, "nor the blank under it");
    let picked: Vec<&str> = rendered
        .picks
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    assert_eq!(
        picked,
        vec!["alpha", "alpha", "alpha", "beta", "beta", "beta"]
    );
}

#[test]
fn the_rail_runs_unbroken_down_to_a_child() {
    // The rows under a name carry the line down to where its first child hangs; without it the
    // tree reads as separate entries rather than one branch.
    let held = vec![agent("alpha", None), agent("beta", Some("alpha"))];
    let rendered = view(&held, &open());
    let under = text(&rendered.rows[3]);
    let child = text(&rendered.rows[5]);
    assert!(under.starts_with('│'), "{under:?}");
    assert!(child.starts_with('└'), "{child:?}");
}

#[test]
fn a_folded_branch_hides_everything_under_it_and_says_how_much() {
    let held = vec![
        agent("alpha", None),
        agent("beta", Some("alpha")),
        agent("gamma", Some("beta")),
    ];
    let folded: BTreeSet<String> = ["alpha".to_owned()].into();
    let rendered = view(&held, &folded);
    assert_eq!(rendered.rows.len(), 2 + 3, "only alpha is drawn");
    assert!(text(&rendered.rows[2]).contains("▸ 2 folded"));
    let under = text(&rendered.rows[3]);
    assert!(!under.starts_with('│'), "no rail to children not drawn");
}

#[test]
fn the_third_row_says_what_the_agent_is_for() {
    let mut held = vec![agent("alpha", None), agent("beta", Some("alpha"))];
    held[1].about = Some("reviews what the others wrote".to_owned());
    let rendered = view(&held, &open());
    assert!(text(&rendered.rows[4]).contains("the root of this run"));
    assert!(text(&rendered.rows[7]).contains("reviews what the others wrote"));
}

/// The whole point of the phase: `finished` reads differently from `idle`, and a coordinator
/// can see it at a glance.
#[test]
fn the_phase_shows_in_the_entry() {
    let said = |a: &Agent| -> String {
        let placed = Placed {
            agent: a,
            head: String::new(),
            body: String::new(),
            hidden: 0,
        };
        entry(&placed)
            .iter()
            .map(text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut a = agent("psi", None);
    a.phase = magi_proto::Phase::Working;
    a.working_for = 5;
    assert!(said(&a).contains('◗') && said(&a).contains("working 5s"));
    a.phase = magi_proto::Phase::Finished;
    assert!(said(&a).contains('✓') && said(&a).contains("finished"));
    a.phase = magi_proto::Phase::Blocked;
    a.cause = Some("run declined".to_owned());
    assert!(said(&a).contains("blocked: run declined"), "{}", said(&a));
    a.phase = magi_proto::Phase::Idle;
    assert!(said(&a).contains('○') && said(&a).contains("idle"));
}
