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
