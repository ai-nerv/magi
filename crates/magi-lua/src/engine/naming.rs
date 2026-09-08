//! Which session a VM belongs to.
//!
//! One test, on purpose: [`super::SESSION`] is a process-global that can be set once, so a second
//! test asserting the absent case would pass or fail on the order the runner happened to pick.
//! Both halves are checked here, in order, where the order is the test's own.

use super::{Engine, balthasar_at, name_session, session};

/// **The two states, in the only sequence that can be observed.** Absent is what a VM nobody named
/// a session for has — `magi tools` builds one, and so does every config test — and a tool that
/// needs the id guards on it rather than inventing one. `config/tools.lua` does exactly that for
/// `history`, which reads this session's own scrollback back out of balthasar.
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

    // **And where its balthasar is**, which is the half that stops a memory tool reaching a
    // neighbour's. balthasar's client takes the newest socket in the directory when nobody says,
    // and the newest is somebody else's as often as not the moment a project has two windows.
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

    // Said once. A second session in one process is not a thing that happens, and refusing it
    // loudly would turn a harmless mistake into a dead window.
    name_session("something-else", None);
    assert_eq!(
        session(),
        Some("00000000001788901214-410e6230cb3b5"),
        "the first name stands"
    );
}
