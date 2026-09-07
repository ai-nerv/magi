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
