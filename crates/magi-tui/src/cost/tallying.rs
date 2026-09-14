//! What the cost view adds up, and what it refuses to guess.

use super::*;

fn turn(at: usize, input: u64, output: u64, read: u64, write: u64) -> Turn {
    Turn {
        at,
        usage: Usage {
            input,
            output,
            cache_read: read,
            cache_write: write,
            cost_micros: 0,
        },
    }
}

fn text(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn nothing_spent_says_so_rather_than_a_table_of_zeroes() {
    assert!(lines(&[], None).is_empty());
    assert!(empty().contains("nothing spent"));
}

#[test]
fn every_turn_is_a_row_and_the_total_is_the_sum() {
    let said = text(&lines(
        &[turn(1, 100, 20, 0, 0), turn(2, 200, 30, 50, 10)],
        None,
    ));
    assert!(said.contains("all"), "there is a total row: {said}");
    // 300 in, 50 out — the sum, not the last turn.
    assert!(said.contains("300"), "{said}");
    assert!(said.contains("50"), "{said}");
}

#[test]
fn the_four_counters_are_kept_apart() {
    let said = text(&lines(&[turn(1, 10, 20, 30, 40)], None));
    for column in ["in", "out", "cache rd", "cache wr"] {
        assert!(said.contains(column), "{column} is a column: {said}");
    }
}

#[test]
fn it_says_how_much_of_the_prompt_was_cached() {
    // 900 of 1000 prompt tokens from cache.
    let said = text(&lines(&[turn(1, 100, 5, 900, 0)], None));
    assert!(said.contains("90% of the prompt"), "{said}");
}

#[test]
fn a_session_with_no_prompt_tokens_does_not_divide_by_zero() {
    let said = text(&lines(&[turn(1, 0, 5, 0, 0)], None));
    assert!(!said.contains("% of the prompt"), "{said}");
}

#[test]
fn it_does_not_invent_a_price() {
    // melchior owns the rate catalog; a rate guessed here would go stale unnoticed.
    let said = text(&lines(&[turn(1, 1_000, 100, 0, 0)], Some("anthropic/x")));
    assert!(!said.contains('$'), "no money: {said}");
    assert!(
        said.contains("providers.lua"),
        "and it says where the rate lives: {said}"
    );
    assert!(said.contains("anthropic/x"), "for this model: {said}");
}

#[test]
fn what_the_provider_said_a_turn_cost_is_shown_and_summed() {
    let mut one = turn(1, 100, 10, 0, 0);
    one.usage.cost_micros = 1_234;
    let mut two = turn(2, 100, 10, 0, 0);
    two.usage.cost_micros = 20_000;
    let said = text(&lines(&[one, two], Some("openrouter/x")));
    assert!(said.contains("cost"), "a money column: {said}");
    assert!(said.contains("$0.0012"), "the first turn: {said}");
    assert!(
        said.contains("spent $0.0212"),
        "and the whole session: {said}"
    );
}

#[test]
fn the_newest_turn_is_last() {
    let said = text(&lines(&[turn(1, 1, 0, 0, 0), turn(2, 2, 0, 0, 0)], None));
    let first = said.find("\n1 ").or_else(|| said.find("1     "));
    let second = said.find("\n2 ").or_else(|| said.find("2     "));
    assert!(first < second, "{said}");
}
