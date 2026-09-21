use super::*;

fn used(input: u64, output: u64, micros: u64) -> magi_proto::Usage {
    magi_proto::Usage {
        input,
        output,
        cost_micros: micros,
        ..magi_proto::Usage::default()
    }
}

fn text_of(drawn: &Rendered) -> String {
    drawn
        .rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn crew_of<'a>(agents: &'a [Agent<'a>]) -> Held<'a> {
    Held { agents, width: 78 }
}

#[test]
fn every_tab_draws_something_rather_than_panicking_on_an_empty_roster() {
    for tab in 0..TABS.len() {
        let drawn = view(&crew_of(&[]), tab);
        assert_eq!(drawn.rows.len(), drawn.picks.len(), "tab {tab}");
        assert!(!empty(tab).is_empty(), "tab {tab}");
    }
}

#[test]
fn a_model_two_agents_reached_is_added_up_once_and_counted_twice() {
    // The whole run's bill, not one screen's: the point of the tab is what the run cost, and an
    // agent-by-agent list cannot be added up by eye.
    let a = vec![("gpt".to_owned(), used(100, 10, 5_000))];
    let b = vec![("gpt".to_owned(), used(200, 20, 7_000))];
    let agents = [
        Agent {
            name: "main/a",
            role: "main",
            here: true,
            phase: "idle",
            claim: None,
            spent: &a,
        },
        Agent {
            name: "kid/b",
            role: "builder",
            here: false,
            phase: "busy",
            claim: None,
            spent: &b,
        },
    ];
    let text = text_of(&view(&crew_of(&agents), 1));
    assert_eq!(text.lines().count(), 1, "one model, one row: {text}");
    assert!(text.contains("2 agents"), "{text}");
    assert!(text.contains("330"), "tokens were not added up: {text}");
}

#[test]
fn who_is_calling_is_named_beside_what_they_spent() {
    let a = vec![
        ("gpt".to_owned(), used(100, 10, 2_000_000)),
        ("claude".to_owned(), used(50, 5, 1_000_000)),
    ];
    let agents = [Agent {
        name: "main/a",
        role: "main",
        here: true,
        phase: "idle",
        claim: None,
        spent: &a,
    }];
    let text = text_of(&view(&crew_of(&agents), 2));
    assert!(text.contains("main/a"), "{text}");
    assert!(
        text.contains("$3.00"),
        "the agent's own total is missing: {text}"
    );
    assert!(text.contains("gpt") && text.contains("claude"), "{text}");
}

#[test]
fn an_agent_that_has_spent_nothing_is_left_out_of_the_bill() {
    let agents = [Agent {
        name: "idle/c",
        role: "main",
        here: false,
        phase: "idle",
        claim: None,
        spent: &[],
    }];
    assert!(view(&crew_of(&agents), 2).rows.is_empty());
}

#[test]
fn the_screen_you_are_on_is_marked_and_what_it_claims_is_shown() {
    let agents = [Agent {
        name: "main/a",
        role: "main",
        here: true,
        phase: "busy",
        claim: Some("rewriting the parser"),
        spent: &[],
    }];
    let drawn = view(&crew_of(&agents), 0);
    let text = text_of(&drawn);
    assert!(text.starts_with('▸'), "{text}");
    assert!(text.contains("rewriting the parser"), "{text}");
    assert_eq!(drawn.picks, vec![Some("main/a".to_owned())]);
}

#[test]
fn a_run_that_cost_nothing_says_so_rather_than_showing_zero_dollars() {
    assert_eq!(money(0), "—");
    assert_eq!(money(500), "<$0.01");
    assert_eq!(money(2_500_000), "$2.50");
}
