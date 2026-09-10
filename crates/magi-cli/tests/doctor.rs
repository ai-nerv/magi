//! `magi doctor`, against the real binary. It answers without starting a session, says where each
//! tool came from, and reports a missing sibling as missing rather than omitting it.

use magi_model::scratch::Scratch;

use std::process::Command;

/// `magi doctor` in a directory of its own, with `PATH` holding only what is passed and
/// `$XDG_CONFIG_HOME` empty.
fn doctor(path: &std::path::Path) -> String {
    let dir = Scratch::new("magi-doctor", "run");
    let config = Scratch::new("magi-doctor", "config");
    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    magi_testkit::only_its_own_store(&mut command);
    let out = command
        .arg("doctor")
        .env("PATH", path)
        .env("XDG_CONFIG_HOME", &*config)
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
fn a_machine_with_no_siblings_says_so_for_each_role() {
    // With none of them installed a session still starts, and silently has no tools or model.
    let empty = Scratch::new("magi-doctor", "empty-path");
    let whole = doctor(&empty);
    // The section, not the report: `model` and `tools` are also a setting and a heading.
    let said = whole
        .split_once("\nroles\n")
        .map(|(_, rest)| rest.to_owned())
        .unwrap_or_else(|| panic!("no roles section:\n{whole}"));

    for (role, program) in [
        ("memory", "balthasar"),
        ("tools", "casper"),
        ("model", "melchior"),
    ] {
        // The role names the program, so the row is findable by both — `tools` is also a heading.
        let line = said
            .lines()
            .find(|line| line.trim_start().starts_with(role) && line.contains(program))
            .unwrap_or_else(|| panic!("{role} is not reported as filled by {program}:\n{said}"));
        assert!(
            line.contains("not installed"),
            "{role} is unfilled and not reported as unfilled: {line}"
        );
    }
}

#[test]
fn the_builtins_are_listed_with_where_they_came_from() {
    // The three compiled-in tools are there whatever else is missing.
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
fn a_machine_with_no_configuration_still_gets_an_answer() {
    // `config::load` reports "no configuration; run `make configs`"; what is compiled in and what
    // is on `$PATH` do not depend on a configuration existing.
    let empty = Scratch::new("magi-doctor", "no-config");
    let said = doctor(&empty);
    assert!(said.contains("will not load"), "it says so: {said}");
    assert!(said.contains("roles"), "and carries on: {said}");
    assert!(
        said.lines()
            .any(|line| line.trim_start().starts_with("read")),
        "the builtins are still listed: {said}"
    );
}

#[test]
fn it_answers_without_starting_a_session() {
    let empty = Scratch::new("magi-doctor", "cold");
    let said = doctor(&empty);
    assert!(said.contains("configuration"), "{said}");
    assert!(said.contains("settings"), "{said}");
    assert!(said.contains("tools"), "{said}");
    assert!(said.contains("roles"), "{said}");
}
