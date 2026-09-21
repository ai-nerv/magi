use super::*;

fn held<'a>(
    memories: &'a [serde_json::Value],
    sessions: &'a [serde_json::Value],
    chosen: Option<&'a str>,
    utility: Option<&'a serde_json::Value>,
    why: Option<&'a serde_json::Value>,
) -> Held<'a> {
    Held {
        laid: None,
        jobs: &[],
        memories: Some(memories),
        chosen,
        utility,
        why,
        sessions: Some(sessions),
        width: 60,
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

#[test]
fn every_tab_draws_something_rather_than_panicking_on_an_empty_layer() {
    // The store is empty on a project's first run, which is exactly when somebody opens this to
    // find out what it does.
    for tab in 0..TABS.len() {
        let drawn = view(&held(&[], &[], None, None, None), tab);
        assert!(drawn.rows.len() == drawn.picks.len(), "tab {tab}");
        assert!(
            !empty(tab, true).is_empty(),
            "tab {tab} says nothing when empty"
        );
    }
}

#[test]
fn a_memory_can_be_picked_so_the_utility_tab_has_something_to_be_about() {
    let rows = vec![serde_json::json!({
        "id": "m-1", "text": "the jail must never cut the network", "kind": "fact",
        "confidence": 0.82,
    })];
    let drawn = view(&held(&rows, &[], None, None, None), 1);
    assert_eq!(drawn.picks, vec![Some("m-1".to_owned())]);
    assert!(text_of(&drawn).contains("never cut the network"));
}

#[test]
fn retrievals_and_outcomes_are_counted_beside_each_other_not_folded_together() {
    // These arrive only where the layer offers `utility`, which is balthasar's own verb and not
    // the memory role's; the tab draws them when they come and the evidence either way.
    // "9" could be nine retrievals or nine confirmations. The layer keeps them apart and so must
    // the screen.
    let use_of = serde_json::json!({
        "times_considered": 12, "times_returned": 9, "helpfulness": 0.75,
        "verified_helpful": 6, "verified_harmful": 1, "ignored": 2, "unknown": 0,
    });
    let drawn = view(&held(&[], &[], Some("m-1"), Some(&use_of), None), 2);
    let text = text_of(&drawn);
    assert!(text.contains("9 returned of 12 considered"), "{text}");
    assert!(text.contains("75%"), "{text}");
    assert!(text.contains("harmed"), "{text}");
}

#[test]
fn a_memory_nothing_has_asserted_says_so_rather_than_drawing_a_blank() {
    let why = serde_json::json!({ "confidence": 0.4, "witnesses": [] });
    let drawn = view(&held(&[], &[], Some("m-1"), None, Some(&why)), 2);
    assert!(text_of(&drawn).contains("nothing has asserted this"));
}

#[test]
fn the_evidence_tab_with_nothing_picked_says_how_to_pick_one() {
    assert!(view(&held(&[], &[], None, None, None), 2).rows.is_empty());
    assert!(empty(2, true).contains("memories tab"));
}

#[test]
fn a_run_still_going_is_marked_and_one_that_ended_is_not() {
    let rows = vec![
        serde_json::json!({ "id": "s-1", "title": "live", "harness": "magi", "open": true }),
        serde_json::json!({ "id": "s-2", "title": "done", "harness": "magi", "open": false }),
    ];
    let drawn = view(&held(&[], &rows, None, None, None), 3);
    let lines: Vec<String> = drawn.rows.iter().map(ToString::to_string).collect();
    assert!(lines[0].starts_with('●'), "{lines:?}");
    assert!(!lines[1].starts_with('●'), "{lines:?}");
    assert_eq!(drawn.picks, vec![Some("s-1".into()), Some("s-2".into())]);
}

#[test]
fn a_long_memory_is_cut_rather_than_breaking_the_frame() {
    let rows = vec![serde_json::json!({ "id": "m", "text": "x ".repeat(400) })];
    let drawn = view(&held(&rows, &[], None, None, None), 1);
    assert!(drawn.rows[0].to_string().chars().count() <= 60, "too wide");
}

#[test]
fn a_session_with_no_title_falls_back_to_its_name() {
    let rows = vec![serde_json::json!({ "id": "s", "name": "hi", "title": "", "open": false })];
    assert!(text_of(&view(&held(&[], &rows, None, None, None), 3)).contains("hi"));
}

#[test]
fn an_empty_store_says_it_is_empty_rather_than_still_asking() {
    // The two look identical on screen otherwise, and a store holding nothing then reads as a
    // layer that never answered.
    assert!(empty(1, false).contains("asking"));
    assert!(!empty(1, true).contains("asking"), "{}", empty(1, true));
    assert!(!empty(3, true).contains("asking"), "{}", empty(3, true));
}

#[test]
fn a_witness_says_when_it_was_said_and_by_whom() {
    // The fields are balthasar's: `at` as an epoch, `session`, and a `note` that falls back to the
    // `kind`. Guessing at their names drew a row of blanks.
    let why = serde_json::json!({
        "confidence": 1.0, "sessions": 1,
        "witnesses": [{ "kind": "imperative", "session": "cli", "at": 1789986698,
                        "note": "typed at the command line" }],
        "against": [],
    });
    let drawn = view(&held(&[], &[], Some("m-1"), None, Some(&why)), 2);
    let text = text_of(&drawn);
    assert!(text.contains("2026-"), "no date: {text}");
    assert!(text.contains("cli"), "{text}");
    assert!(text.contains("typed at the command line"), "{text}");
    assert!(text.contains("1 session"), "{text}");
}

#[test]
fn a_witness_with_no_note_falls_back_to_what_sort_it_is() {
    let why = serde_json::json!({
        "witnesses": [{ "kind": "imperative", "session": "cli", "at": 1789986698 }],
    });
    assert!(text_of(&view(&held(&[], &[], Some("m"), None, Some(&why)), 2)).contains("imperative"));
}

#[test]
fn evidence_against_a_memory_is_shown_rather_than_left_out() {
    // A memory something contradicts is exactly the one worth looking at.
    let why = serde_json::json!({
        "witnesses": [{ "kind": "said", "session": "a", "at": 1789986698 }],
        "against": [{ "kind": "said", "session": "b", "at": 1789986700, "note": "the opposite" }],
    });
    let text = text_of(&view(&held(&[], &[], Some("m"), None, Some(&why)), 2));
    assert!(text.contains("against"), "{text}");
    assert!(text.contains("the opposite"), "{text}");
}
