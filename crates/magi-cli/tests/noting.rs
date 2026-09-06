//! What a spawned sibling said, when somebody asked to be told.
//!
//! Every crossing in this program nulls its child's stderr and turns a failure into an empty
//! answer, because a line on stderr lands in the middle of a frame. The cost is that `magi
//! models` printing nothing means any of four things and says which of them it was to nobody.
//!
//! `MAGI_DEBUG_LOG` is the way out, and it is only worth having if it works at a real crossing
//! in a real process. Set on the child rather than on the runner: the whole mechanism reads the
//! environment the binary was started with, and a test that set it in-process could not run
//! alongside one that did not.

use magi_model::scratch::Scratch;

use std::process::Command;

#[test]
fn a_sibling_that_will_not_start_says_so_in_the_log() {
    // `PATH` is one empty directory, so there is no melchior anywhere. This is the case a person
    // hits on a fresh machine, and the one where an empty model list is least informative.
    let dir = Scratch::new("magi-noting", "missing");
    let log = dir.join("debug.log");
    let empty = dir.join("bin");
    std::fs::create_dir_all(&empty).expect("mkdir");

    let out = Command::new(env!("CARGO_BIN_EXE_magi"))
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
    // The default. A tool that left a file on disk because it once failed would fill one.
    let dir = Scratch::new("magi-noting", "quiet");
    let log = dir.join("debug.log");
    let empty = dir.join("bin");
    std::fs::create_dir_all(&empty).expect("mkdir");

    let out = Command::new(env!("CARGO_BIN_EXE_magi"))
        .arg("models")
        .env("PATH", &empty)
        .env_remove("MAGI_DEBUG_LOG")
        .current_dir(&*dir)
        .output()
        .expect("magi models runs");

    assert!(out.status.success());
    assert!(!log.exists(), "{}", log.display());
}
