//! Naming a session, now that nothing on disk names one.

use super::session_id;

/// Ids sort by time because they are timestamps.
#[test]
fn ids_sort_in_the_order_the_sessions_started() {
    let mut ids = [
        session_id(1_788_427_145, ""),
        session_id(1_787_863_714, ""),
        session_id(1_788_398_118, ""),
    ];
    ids.sort();
    assert_eq!(ids[0], session_id(1_787_863_714, ""));
    assert_eq!(ids[2], session_id(1_788_427_145, ""));
}

/// Two sessions started in the same second still get different names.
#[test]
fn two_sessions_started_in_the_same_second_are_still_told_apart() {
    assert_ne!(
        session_id(1_788_427_145, "a1b2c3"),
        session_id(1_788_427_145, "d4e5f6")
    );
}

#[test]
fn an_unqualified_id_is_just_the_time() {
    assert_eq!(session_id(7, ""), "00000000000000000007");
}

#[test]
fn a_childs_own_transcript_is_not_offered_to_resume() {
    // `run@agent` is what a subagent wrote. Only a run is resumed, and it brings its agents back
    // itself; offering each of them turned the picker into a list of every child ever spawned.
    let rows = [
        serde_json::json!({"id": "psi-lambda-1@chi-omega", "title": "an audit"}),
        serde_json::json!({"id": "psi-lambda-1", "title": "hi"}),
    ];
    let offered: Vec<String> = rows
        .iter()
        .filter_map(super::summary_of)
        .map(|found| found.id)
        .collect();
    assert_eq!(offered, ["psi-lambda-1"]);
}
