//! What a spawned sibling said, when somebody asked to be told. `MAGI_DEBUG_LOG` is set on the
//! child rather than on the runner: the mechanism reads the environment the binary was started
//! with, so a test setting it in-process could not run alongside one that did not.

use magi_model::scratch::Scratch;

use std::process::Command;

#[test]
fn a_sibling_that_will_not_start_says_so_in_the_log() {
    // `PATH` is one empty directory, so there is no melchior anywhere.
    let dir = Scratch::new("magi-noting", "missing");
    let log = dir.join("debug.log");
    let empty = dir.join("bin");
    std::fs::create_dir_all(&empty).expect("mkdir");

    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    magi_testkit::only_its_own_store(&mut command);
    let out = command
        .arg("models")
        .env("PATH", &empty)
        .env("MAGI_DEBUG_LOG", &log)
        .current_dir(&*dir)
        .output()
        .expect("magi models runs");

    assert!(out.status.success(), "an absent sibling is not a crash");
    let held = std::fs::read_to_string(&log).expect("the log was written");
    assert!(
        held.contains("melchior could not be started"),
        "the log says which of the four it was: {held}"
    );
}

#[test]
fn nothing_is_written_when_nobody_asked() {
    let dir = Scratch::new("magi-noting", "quiet");
    let log = dir.join("debug.log");
    let empty = dir.join("bin");
    std::fs::create_dir_all(&empty).expect("mkdir");

    let mut command = Command::new(env!("CARGO_BIN_EXE_magi"));
    magi_testkit::only_its_own_store(&mut command);
    let out = command
        .arg("models")
        .env("PATH", &empty)
        .env_remove("MAGI_DEBUG_LOG")
        .current_dir(&*dir)
        .output()
        .expect("magi models runs");

    assert!(out.status.success());
    assert!(!log.exists(), "{}", log.display());
}
