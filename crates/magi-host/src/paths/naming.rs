//! Naming a session, now that nothing on disk names one.

use super::session_id;

/// Ids sort by time because they are timestamps, which is what makes "newest first" an ordering
/// rather than a search.
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

/// **Seconds are not enough on their own.** Starting a second `magi` beside the first is the
/// ordinary way to get two, and a person doing it does not pause a second first — so two sessions
/// took the same name and wrote into it together.
#[test]
fn two_sessions_started_in_the_same_second_are_still_told_apart() {
    assert_ne!(
        session_id(1_788_427_145, "a1b2c3"),
        session_id(1_788_427_145, "d4e5f6")
    );
}

/// With nobody to be told apart from, the timestamp alone is the name.
#[test]
fn an_unqualified_id_is_just_the_time() {
    assert_eq!(session_id(7, ""), "00000000000000000007");
}
