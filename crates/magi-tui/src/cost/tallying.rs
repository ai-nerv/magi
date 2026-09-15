//! What the cost view adds up, and what it refuses to guess.

use super::*;

fn used(input: u64, output: u64, cost_micros: u64) -> Usage {
    Usage {
        input,
        output,
        cost_micros,
        ..Usage::default()
    }
}

fn turn(at: usize, model: &str, usage: Usage) -> Turn {
    Turn {
        at,
        model: model.to_owned(),
        usage,
    }
}

fn text(drawn: &Rendered) -> Vec<String> {
    drawn.rows.iter().map(ToString::to_string).collect()
}

fn report<'a>(turns: &'a [Turn], agents: &'a [Agent]) -> Report<'a> {
    Report {
        turns,
        agents,
        helpers: &[],
        width: 70,
    }
}

#[test]
fn helper_jobs_get_a_section_and_count_toward_the_heading() {
    let turns = [turn(1, "p/big", used(1000, 100, 20_000))];
    let helpers = [Helper {
        role: "memory".into(),
        model: "p/small".into(),
        usage: used(800, 50, 1_000),
    }];
    let shown = Report {
        helpers: &helpers,
        ..report(&turns, &[])
    };
    let all = text(&view(&shown)).join("\n");
    assert!(all.contains("Helpers"), "{all}");
    assert!(all.contains("memory · p/small"), "{all}");
    assert!(all.contains("helpers $0.0010"), "{all}");
}

/// Whether any row carries a braille cell: a line chart was drawn.
fn charted(shown: &[String]) -> bool {
    shown
        .iter()
        .any(|row| row.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)))
}

#[test]
fn nothing_spent_draws_nothing_and_says_so() {
    assert!(view(&report(&[], &[])).rows.is_empty());
    assert!(empty().contains("nothing spent"));
}

#[test]
fn the_heading_is_the_whole_of_what_was_spent() {
    let turns = [
        turn(1, "m/alpha", used(100, 10, 1_234)),
        turn(2, "m/alpha", used(100, 10, 20_000)),
    ];
    let shown = text(&view(&report(&turns, &[])));
    assert_eq!(shown[0], "$0.0212 spent", "{shown:?}");
}

#[test]
fn each_model_is_a_bar_of_its_own() {
    let turns = [
        turn(1, "openrouter/x/alpha", used(1_000, 100, 1_000)),
        turn(2, "openrouter/y/beta", used(2_000, 100, 3_000)),
    ];
    let shown = text(&view(&report(&turns, &[])));
    let has = |needle: &str| shown.iter().any(|row| row.contains(needle));
    assert!(has("By model") && has("alpha") && has("beta"), "{shown:?}");
    assert!(has("█"), "drawn as bars: {shown:?}");
    assert!(has("2 models"), "{shown:?}");
}

#[test]
fn every_agent_of_the_run_is_counted() {
    let turns = [turn(1, "m/alpha", used(1_000, 100, 1_000))];
    let agents = [
        Agent {
            name: "main/xi".to_owned(),
            here: true,
            spent: vec![("m/alpha".to_owned(), used(1_000, 100, 1_000))],
        },
        Agent {
            name: "builder/pi".to_owned(),
            here: false,
            spent: vec![("m/beta".to_owned(), used(2_000, 200, 4_000))],
        },
    ];
    let shown = text(&view(&report(&turns, &agents)));
    let has = |needle: &str| shown.iter().any(|row| row.contains(needle));
    assert_eq!(shown[0], "$0.0050 spent", "the whole run: {shown:?}");
    assert!(
        has("By agent") && has("builder/pi") && has("you"),
        "{shown:?}"
    );
    assert!(has("beta"), "a model only a child used: {shown:?}");
}

#[test]
fn it_does_not_invent_a_price() {
    // melchior owns the rate catalog; a rate guessed here would go stale unnoticed.
    let turns = [turn(1, "anthropic/x", used(1_000, 100, 0))];
    let shown = text(&view(&report(&turns, &[])));
    assert!(shown.iter().all(|row| !row.contains('$')), "{shown:?}");
    assert!(shown[0].ends_with("tokens"), "{shown:?}");
}

#[test]
fn how_much_came_from_cache_is_a_gauge() {
    let cached = Usage {
        cache_read: 900,
        ..used(100, 5, 0)
    };
    let shown = text(&view(&report(&[turn(1, "m/a", cached)], &[])));
    assert!(
        shown
            .iter()
            .any(|row| row.contains("90% of the prompt from cache")),
        "{shown:?}"
    );
}

#[test]
fn turns_are_charted_and_listed_with_their_model() {
    let turns = [
        turn(1, "m/alpha", used(1_000, 100, 1_000)),
        turn(2, "m/alpha", used(3_000, 100, 2_000)),
        turn(3, "m/beta", used(2_000, 100, 1_000)),
    ];
    let shown = text(&view(&report(&turns, &[])));
    let has = |needle: &str| shown.iter().any(|row| row.contains(needle));
    assert!(
        has("Tokens per turn") && has("turn 1") && has("turn 3"),
        "{shown:?}"
    );
    assert!(charted(&shown), "the spend over time: {shown:?}");
    assert!(has("Recent turns"), "{shown:?}");
    let listed = shown.iter().position(|row| row.contains("Recent turns"));
    let last = shown.iter().rposition(|row| row.contains("beta"));
    assert!(listed < last, "the newest turn is last: {shown:?}");
}

#[test]
fn no_row_is_wider_than_the_view() {
    let turns: Vec<Turn> = (1..=40)
        .map(|at| {
            turn(
                at,
                "openrouter/some-provider/a-very-long-model-name",
                used(9_000, 900, 900),
            )
        })
        .collect();
    for row in text(&view(&report(&turns, &[]))) {
        assert!(row.chars().count() <= 70, "{row:?}");
    }
}
