//! `magi doctor`, against the real binary.
//!
//! Everything this command answers is decided at start-up and was previously discoverable only
//! by starting a session and noticing an absence. The properties worth holding are that it
//! answers without one, that it says where each tool came from, and that a sibling which is not
//! there is reported as not there rather than omitted — an empty list and a missing program look
//! identical, and telling them apart is most of why somebody runs this.

use magi_model::scratch::Scratch;

use std::process::Command;

/// `magi doctor` in a directory of its own, with `PATH` holding only what is passed.
fn doctor(path: &std::path::Path) -> String {
    let dir = Scratch::new("magi-doctor", "run");
    let out = Command::new(env!("CARGO_BIN_EXE_magi"))
        .arg("doctor")
        .env("PATH", path)
        .current_dir(&*dir)
        .output()
        .expect("magi doctor runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_machine_with_no_siblings_says_so_for_each_of_them() {
    // The case the command exists for. With none of them installed a session still starts, and
    // silently has no tools, no model and no memory.
    let empty = Scratch::new("magi-doctor", "empty-path");
    let said = doctor(&empty);

    for name in ["casper", "melchior", "balthasar"] {
        let line = said
            .lines()
            .find(|line| line.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("{name} is not mentioned at all:\n{said}"));
        assert!(
            line.contains("not installed"),
            "{name} is absent and not reported as absent: {line}"
        );
    }
}

#[test]
fn the_builtins_are_listed_with_where_they_came_from() {
    // The three that are compiled in are there whatever else is missing, and a listing that did
    // not say where a tool came from could not distinguish those from a config's own.
    let empty = Scratch::new("magi-doctor", "builtins");
    let said = doctor(&empty);

    for name in ["read", "write", "edit"] {
        let line = said
            .lines()
            .find(|line| line.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("{name} is missing:\n{said}"));
        assert!(line.contains("builtin"), "{line}");
    }
}

#[test]
fn it_answers_without_starting_a_session() {
    // No socket, no daemon, no model. If this ever needs one, the command has stopped being
    // usable for the case it was written for: a machine where the session will not start.
    let empty = Scratch::new("magi-doctor", "cold");
    let said = doctor(&empty);
    assert!(said.contains("configuration"), "{said}");
    assert!(said.contains("settings"), "{said}");
    assert!(said.contains("tools"), "{said}");
    assert!(said.contains("siblings"), "{said}");
}
