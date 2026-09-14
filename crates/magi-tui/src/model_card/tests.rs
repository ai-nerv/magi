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
        turns,
        details,
        width: 70,
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
            Endpoint {
                provider: "Slow".to_owned(),
                quantization: None,
                uptime: Some(90.0),
            },
            Endpoint {
                provider: "DeepInfra".to_owned(),
                quantization: Some("fp4".to_owned()),
                uptime: Some(99.5),
            },
        ],
    }
}

/// Whether any row carries a braille cell: a line chart was drawn.
fn charted(shown: &[String]) -> bool {
    shown
        .iter()
        .any(|row| row.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)))
}

#[test]
fn it_says_which_model_and_where_it_comes_from() {
    let shown = text(&view(&card(true, &[], Known::Asking)));
    assert_eq!(shown[0], "deepseek/deepseek-v4");
    assert!(shown[1].contains("openrouter"), "{shown:?}");
    assert!(shown[1].contains("128k context"), "{shown:?}");
}

#[test]
fn only_the_settings_can_be_taken() {
    let details = published();
    let turns = [turn(1_000, 100, 10)];
    let drawn = view(&card(true, &turns, Known::Found(&details)));
    let picked: Vec<&str> = drawn.picks.iter().flatten().map(String::as_str).collect();
    assert_eq!(picked, ["thinking", "switch"]);
    assert_eq!(drawn.rows.len(), drawn.picks.len());
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
fn what_the_provider_publishes_is_charted() {
    let details = published();
    let shown = text(&view(&card(true, &[], Known::Found(&details))));
    let has = |needle: &str| shown.iter().any(|row| row.contains(needle));
    assert!(has("A fast model for code."), "{shown:?}");
    assert!(has("Pricing") && has("$0.30") && has("$1.20"), "{shown:?}");
    assert!(has("up to 32k"), "{shown:?}");
    assert!(has("2025-06"), "{shown:?}");
    assert!(has("Benchmarks") && has("coding"), "{shown:?}");
    assert!(has("█"), "the bars are drawn: {shown:?}");
    assert!(has("99.50%"), "{shown:?}");
    let first = shown.iter().position(|row| row.contains("DeepInfra"));
    let second = shown.iter().position(|row| row.contains("Slow"));
    assert!(first < second, "most reliable first: {shown:?}");
}

#[test]
fn a_session_with_no_turns_says_so() {
    let shown = text(&view(&card(true, &[], Known::Asking)));
    assert!(
        shown.iter().any(|row| row.contains("nothing yet")),
        "{shown:?}"
    );
    assert!(!charted(&shown), "no chart of nothing");
}

#[test]
fn a_session_is_drawn_as_columns_a_gauge_and_a_line() {
    let turns = [
        turn(10_000, 100, 0),
        turn(40_000, 400, 0),
        turn(20_000, 200, 0),
    ];
    let shown = text(&view(&card(true, &turns, Known::Asking)));
    let has = |needle: &str| shown.iter().any(|row| row.contains(needle));
    assert!(has("3 turns"), "{shown:?}");
    assert!(
        has("context 16% of 128k"),
        "the gauge, off the last turn: {shown:?}"
    );
    assert!(has("tokens per turn") && has("█"), "the columns: {shown:?}");
    assert!(charted(&shown), "the window's line: {shown:?}");
    assert!(!has("spent"), "no money unsaid");
}

#[test]
fn money_is_shown_and_charted_where_the_provider_said_it() {
    let turns = [turn(1_000, 100, 1_500), turn(1_000, 100, 2_500)];
    let shown = text(&view(&card(true, &turns, Known::Asking)));
    assert!(
        shown.iter().any(|row| row.contains("$0.0040 spent")),
        "{shown:?}"
    );
    assert!(
        shown
            .iter()
            .any(|row| row.contains("money, as it added up")),
        "{shown:?}"
    );
}

/// Not a check: prints a whole card, to look at. `cargo test -p magi-tui a_card_to_look_at -- --ignored --nocapture`.
#[test]
#[ignore = "prints a card to look at"]
fn a_card_to_look_at() {
    let details = Details {
        description: Some("DeepSeek V4 Flash is a sparse mixture-of-experts model, 13B active out of 284B, suited to coding, reasoning and agent workflows.".to_owned()),
        knowledge_cutoff: Some("2025-12-01".to_owned()),
        modality: Some("text->text".to_owned()),
        max_output: Some(943_718),
        price: [0.06, 0.12, 0.012, 0.0],
        benchmarks: vec![
            ("intelligence".to_owned(), 34.5),
            ("coding".to_owned(), 69.1),
            ("agentic".to_owned(), 41.7),
        ],
        endpoints: ["BaseTen:fp8:99.95", "Morph:bf16:99.93", "DeepInfra:fp8:99.92", "Reka:fp4:99.34", "StreamLake:fp8:95.18", "Makora::97.64"]
            .iter()
            .map(|spec| {
                let parts: Vec<&str> = spec.split(':').collect();
                Endpoint {
                    provider: parts[0].to_owned(),
                    quantization: (!parts[1].is_empty()).then(|| parts[1].to_owned()),
                    uptime: parts[2].parse().ok(),
                }
            })
            .collect(),
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
