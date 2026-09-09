//! Which session a VM belongs to.
//!
//! One test on purpose: [`super::SESSION`] is a process-global set once, so both halves are here.

use super::{Engine, balthasar_at, name_session, session};

/// The two states, in the only sequence that can be observed. A tool that needs the id guards on
/// the absent case rather than inventing one.
#[test]
fn a_vm_learns_which_session_it_is_once_somebody_says() {
    assert!(session().is_none(), "nothing has named a session yet");

    let mut before = Engine::new();
    before
        .run(
            "assert(magi.session == nil, 'an unnamed VM should not claim a session')",
            "before",
        )
        .expect("the unnamed VM");

    name_session(
        "00000000001788901214-410e6230cb3b5",
        Some(std::path::Path::new(
            "/run/user/1000/balthasar/api@ours.sock",
        )),
    );
    assert_eq!(session(), Some("00000000001788901214-410e6230cb3b5"));

    let mut after = Engine::new();
    after
        .run(
            "assert(magi.session == '00000000001788901214-410e6230cb3b5', magi.session or 'nil')",
            "after",
        )
        .expect("the named VM");

    // And where its balthasar is, which is what stops a memory tool reaching a neighbour's: the
    // client takes the newest socket in the directory when nobody says.
    assert_eq!(
        balthasar_at(),
        Some("/run/user/1000/balthasar/api@ours.sock")
    );
    after
        .run(
            "assert(magi.balthasar_at == '/run/user/1000/balthasar/api@ours.sock', \
             magi.balthasar_at or 'nil')",
            "socket",
        )
        .expect("the named VM knows its socket");

    // Said once. A second session in one process is not a thing that happens.
    name_session("something-else", None);
    assert_eq!(
        session(),
        Some("00000000001788901214-410e6230cb3b5"),
        "the first name stands"
    );
}
