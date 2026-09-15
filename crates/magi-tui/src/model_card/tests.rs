//! What a card says, which of its rows can be taken, and that its charts are drawn.
//!
//! Split out under THE RULE; the card these check is next door.

use super::*;

fn text(rendered: &Rendered) -> Vec<String> {
    rendered.rows.iter().map(ToString::to_string).collect()
}

fn turn(input: u64, output: u64, cost_micros: u64) -> Usage {
    Usage {
        input,
        output,
        cost_micros,
        ..Usage::default()
    }
}

fn card<'a>(reasons: bool, turns: &'a [Usage], details: Known<'a>) -> Card<'a> {
    Card {
        model: "openrouter/deepseek/deepseek-v4",
        context_window: 128_000,
        reasons,
        thinking: "medium",
        provider: None,
        turns,
        details,
        sent: None,
        split: None,
        width: 70,
    }
}

#[test]
fn a_laid_out_request_is_said_under_the_context_gauge() {
    let turns = [turn(64_000, 10, 0)];
    let mut shown = card(false, &turns, Known::Asking);
    shown.sent = Some("9 whole · 2 stubbed");
    shown.split = Some("conversation 62% · summary 8% · fixed 11k");
    let all = text(&view(&shown)).join("\n");
    assert!(all.contains("9 whole · 2 stubbed"), "{all}");
}

fn serving(provider: &str, tag: &str, price: [f64; 4]) -> Endpoint {
    Endpoint {
        provider: provider.to_owned(),
        tag: Some(tag.to_owned()),
        price,
        context: Some(128_000),
        ..Endpoint::default()
    }
}

fn published() -> Details {
    Details {
        description: Some("A fast model for code.".to_owned()),
        knowledge_cutoff: Some("2025-06".to_owned()),
        modality: Some("text->text".to_owned()),
        max_output: Some(32_000),
        price: [0.3, 1.2, 0.03, 0.0],
        benchmarks: vec![("coding".to_owned(), 50.0)],
        endpoints: vec![
            serving("Slow", "slow", [0.5, 2.0, 0.0, 0.0]),
            Endpoint {
                quantization: Some("fp4".to_owned()),
                throughput: Some(85.0),
                ..serving("DeepInfra", "deepinfra/fp4", [0.3, 1.2, 0.0, 0.0])
            },
        ],
        tokenizer: Some("DeepSeek".to_owned()),
        inputs: vec!["text".to_owned()],
        outputs: vec!["text".to_owned()],
        features: vec!["tools".to_owned(), "reasoning".to_owned()],
        created: Some(1_700_000_000),
        moderated: Some(false),
    }
}

#[test]
fn it_says_which_model_and_where_it_comes_from() {
    let shown = text(&view(&card(true, &[], Known::Asking)));
    assert_eq!(shown[0], "deepseek/deepseek-v4");
    assert!(shown[1].contains("openrouter"), "{shown:?}");
    assert!(shown[1].contains("128k context"), "{shown:?}");
}

#[test]
fn every_section_is_set_off_by_a_dashed_rule() {
    let details = published();
    let shown = text(&view(&card(
        true,
        &[turn(1_000, 100, 0)],
        Known::Found(&details),
    )));
    let rules = shown.iter().filter(|row| row.starts_with("- - - ")).count();
    assert!(rules >= 6, "{rules} rules in {shown:?}");
}

#[test]
fn only_the_settings_and_the_providers_can_be_taken() {
    let details = published();
    let turns = [turn(1_000, 100, 10)];
    let drawn = view(&card(true, &turns, Known::Found(&details)));
    let picked: Vec<&str> = drawn.picks.iter().flatten().map(String::as_str).collect();
    assert_eq!(
        picked,
        [
            "thinking",
            "switch",
            "provider:",
            "provider:deepinfra/fp4",
            "provider:slow"
        ],
        "the router first, then the cheapest"
    );
    assert_eq!(drawn.rows.len(), drawn.picks.len());
}

#[test]
fn the_provider_serving_it_is_the_one_marked() {
    let details = published();
    let mut chosen = card(true, &[], Known::Found(&details));
    chosen.provider = Some("slow");
    let shown = text(&view(&chosen));
    let row = |name: &str| shown.iter().find(|row| row.contains(name)).cloned();
    assert!(
        row("Slow").is_some_and(|row| row.starts_with('◉')),
        "{shown:?}"
    );
    assert!(
        row("auto").is_some_and(|row| row.starts_with('○')),
        "{shown:?}"
    );
}

#[test]
fn the_thinking_level_is_shown_as_a_value_to_step() {
    let shown = text(&view(&card(true, &[], Known::Asking)));
    assert!(
        shown.iter().any(|row| row.contains("◂ medium ▸")),
        "{shown:?}"
    );
    let refused = text(&view(&card(false, &[], Known::Asking)));
    assert!(
        refused.iter().any(|row| row.contains("does not reason")),
        "{refused:?}"
    );
}

#[test]
fn it_says_while_it_is_asking_and_why_when_it_cannot() {
    let asking = text(&view(&card(true, &[], Known::Asking)));
    assert!(
        asking.iter().any(|row| row.contains("asking the provider")),
        "{asking:?}"
    );
    let missing = text(&view(&card(true, &[], Known::Missing("not published"))));
    assert!(
        missing.iter().any(|row| row.contains("not published")),
        "{missing:?}"
    );
}

#[test]
fn what_is_published_is_shown_and_charted() {
    let details = published();
    let shown = text(&view(&card(true, &[], Known::Found(&details))));
    let has = |needle: &str| shown.iter().any(|row| row.contains(needle));
    assert!(has("A fast model for code."), "{shown:?}");
    assert!(has("Price") && has("$0.30") && has("$1.20"), "{shown:?}");
    assert!(has("up to 32k"), "{shown:?}");
    assert!(
        has("2025-06") && has("text → text") && has("DeepSeek"),
        "{shown:?}"
    );
    assert!(has("2023-11-14"), "released: {shown:?}");
    assert!(has("tools · reasoning"), "{shown:?}");
    assert!(has("Benchmarks") && has("coding"), "{shown:?}");
    assert!(has("█"), "the bars are drawn: {shown:?}");
    assert!(has("85 t/s"), "{shown:?}");
    let first = shown.iter().position(|row| row.contains("DeepInfra"));
    let second = shown.iter().position(|row| row.contains("Slow"));
    assert!(first < second, "cheapest first: {shown:?}");
}

#[test]
fn a_date_is_read_off_the_epoch() {
    assert_eq!(published::date(0), "1970-01-01");
    assert_eq!(published::date(1_700_000_000), "2023-11-14");
    assert_eq!(published::date(951_782_400), "2000-02-29");
}

#[test]
fn how_full_the_window_is_stays_and_what_was_spent_goes_to_the_cost_view() {
    let turns = [turn(10_000, 100, 900), turn(20_000, 200, 900)];
    let shown = text(&view(&card(true, &turns, Known::Asking)));
    assert!(
        shown.iter().any(|row| row.contains("context 16% of 128k")),
        "the gauge, off the last turn: {shown:?}"
    );
    assert!(
        shown
            .iter()
            .all(|row| !row.contains("Tokens per turn") && !row.contains("Spent")),
        "{shown:?}"
    );
}

/// Not a check: prints a whole card, to look at. `cargo test -p magi-tui a_card_to_look_at -- --ignored --nocapture`.
#[test]
#[ignore = "prints a card to look at"]
fn a_card_to_look_at() {
    let details = Details {
        benchmarks: vec![
            ("intelligence".to_owned(), 34.5),
            ("coding".to_owned(), 69.1),
            ("agentic".to_owned(), 41.7),
        ],
        features: [
            "tools",
            "reasoning",
            "structured_outputs",
            "response_format",
            "tool_choice",
            "parallel_tool_calls",
            "seed",
        ]
        .map(ToOwned::to_owned)
        .to_vec(),
        ..published()
    };
    let turns: Vec<Usage> = (1..=14u64)
        .map(|n| {
            turn(
                8_000 * n + 3_000 * (n % 3),
                400 + 90 * (n % 5),
                700 + 150 * n,
            )
        })
        .collect();
    let mut shown = card(true, &turns, Known::Found(&details));
    shown.width = 76;
    for row in text(&view(&shown)) {
        println!("│ {row}");
    }
}

#[test]
fn no_row_is_wider_than_the_card() {
    let details = published();
    let turns = [turn(10_000, 100, 1_000); 30];
    for row in text(&view(&card(true, &turns, Known::Found(&details)))) {
        assert!(row.chars().count() <= 70, "{row:?}");
    }
}
