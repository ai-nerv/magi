//! The coordinated half of the contract, against siblings that are actually installed.
//!
//! `needs` and `configure` are two programs agreeing about what a name means, and the only way
//! that agreement can be checked is by asking the other program. Everything in `driving`'s own
//! tests is magi agreeing with magi: they pin the Lua that gets written, not whether anybody
//! accepts it — which is why casper could send its declarations in a shape magi could not read
//! for as long as it did, with both sides' tests green.
//!
//! Skipped when a sibling is not installed. `MAGI_REQUIRE_LIVE=1` turns that skip into a failure,
//! for the run where you know they are there and want to be held to it.

use magi_host::driving;

/// Whether to insist. See the module docs.
fn required() -> bool {
    std::env::var("MAGI_REQUIRE_LIVE").is_ok_and(|value| value == "1")
}

/// Skip, or fail, depending.
fn missing(program: &str) -> bool {
    assert!(
        !required(),
        "{program} is not installed and MAGI_REQUIRE_LIVE=1"
    );
    eprintln!("skipping: {program} is not installed");
    true
}

fn absent(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("verbs")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
        && missing(program)
}

#[tokio::test]
async fn every_sibling_declares_what_it_takes_in_a_shape_magi_can_read() {
    for program in ["casper", "melchior", "balthasar"] {
        if absent(program) {
            continue;
        }
        let needs = driving::needs(program).await;
        assert!(
            !needs.is_empty(),
            "{program} declares nothing magi could parse — the rows are probably wrapped"
        );
        for need in &needs {
            assert!(!need.name.is_empty(), "{program}: a setting with no name");
            assert!(
                !need.about.is_empty(),
                "{program}: {} says nothing about itself",
                need.name
            );
        }
    }
}

#[tokio::test]
async fn a_setting_nobody_declared_comes_back_refused_by_name() {
    // The half that makes a rename survivable. A sibling that ignored what it did not recognise
    // would leave a misspelled setting looking exactly like an applied one.
    for program in ["casper", "melchior", "balthasar"] {
        if absent(program) {
            continue;
        }
        let applied = driving::configure(program, &format!("{program}.nonesuch_probe = 3\n"))
            .await
            .unwrap_or_else(|why| panic!("{program} would not take a chunk at all: {why}"));
        assert!(
            applied.refused.iter().any(|r| r.name == "nonesuch_probe"),
            "{program} did not name what it refused: {applied:?}"
        );
    }
}

#[tokio::test]
async fn a_table_setting_survives_the_round_trip() {
    // casper is the sibling whose settings are tables, and the reason the coordinator had to
    // learn to write them. This is the whole loop: ask what it takes, write the Lua for it, hand
    // it over, and read back that it was set rather than refused.
    if absent("casper") {
        return;
    }
    let needs = driving::needs("casper").await;
    assert!(
        needs.iter().any(|need| need.name == "tools"),
        "casper stopped declaring `tools`: {needs:?}"
    );
    let source = driving::saying(
        "casper",
        &needs,
        &[("tools", serde_json::json!({ "dino": { "off": true } }))],
    );
    assert!(
        source.contains("casper.tools = {"),
        "wrote nothing: {source}"
    );

    let applied = driving::configure("casper", &source)
        .await
        .unwrap_or_else(|why| panic!("casper refused the chunk: {why}"));
    assert!(
        applied.set.iter().any(|name| name == "tools"),
        "casper did not take it: {applied:?}"
    );
    assert!(applied.refused.is_empty(), "{applied:?}");
}

#[tokio::test]
async fn what_a_coordinator_says_reaches_a_sibling_it_spawns_per_call() {
    // **The hole `configure` alone left.** casper is one process per call, so a `configure` that
    // set something in the process answering it reported the setting taken and changed nothing:
    // every later `casper tools` and `casper run` was a fresh process that knew nothing about it.
    // melchior and balthasar do not have this problem — they are asked once and then run.
    //
    // So the settings ride on every spawn, and this is the test that says they arrive.
    if absent("casper") {
        return;
    }
    let all = magi_tools::casper::cards_from("casper");
    assert!(!all.is_empty(), "casper offers nothing at all");

    let off = magi_tools::casper::cards_configured("casper", r#"{"tools":{"dino":{"off":true}}}"#);
    assert!(
        all.iter().any(|card| card.name == "dino"),
        "dino is there by default: {:?}",
        all.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    assert!(
        !off.iter().any(|card| card.name == "dino"),
        "and switched off by what magi told it: {:?}",
        off.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    assert_eq!(off.len(), all.len() - 1, "and nothing else moved");
}

#[tokio::test]
async fn a_setting_a_sibling_would_refuse_is_named_before_it_is_relied_on() {
    // What `configure` is still for on a spawn-per-call program: a dry run. A coordinator wants
    // to know *which* of its settings would be refused before it commits to sending them on
    // every spawn, and "refused" without a name is not something it can act on.
    if absent("casper") {
        return;
    }
    let applied = driving::configure(
        "casper",
        "casper.nonesuch_probe = 3\ncasper.output_bytes = 4096\n",
    )
    .await
    .expect("casper takes a chunk");
    assert!(
        applied.refused.iter().any(|r| r.name == "nonesuch_probe"),
        "{applied:?}"
    );
    assert!(
        applied.set.iter().any(|name| name == "output_bytes"),
        "and says which ones it would take: {applied:?}"
    );
}
