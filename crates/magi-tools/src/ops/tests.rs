use super::*;
use magi_model::scratch::Scratch;

fn rooted(name: &str) -> (Real, Scratch) {
    let dir = Scratch::new("magi-ops", name);
    (Real::new(dir.to_path_buf()), dir)
}

#[test]
fn a_jails_shared_tmp_is_on_disk_in_the_users_cache() {
    let at = |cache: &str, home: &str| {
        let given = |value: &str| (!value.is_empty()).then(|| OsString::from(value));
        scratch_under(given(cache), given(home))
    };
    assert_eq!(at("/x/cache", "/home/u"), Some(PathBuf::from("/x/cache")));
    assert_eq!(at("", "/home/u"), Some(PathBuf::from("/home/u/.cache")));
    assert_eq!(at("relative", ""), None);
    assert_eq!(at("", ""), None);
}

#[test]
fn isolation_off_is_no_jail_profile_at_all() {
    let (ops, _dir) = rooted("no-jail");
    assert_eq!(
        ops.jail(),
        None,
        "a session that did not ask for a jail gets none"
    );
}

#[test]
fn a_write_grant_over_home_or_a_credential_is_not_handed_to_the_jail() {
    let home = Scratch::new("magi-ops", "home");
    std::fs::create_dir_all(home.join("work")).expect("mkdir");
    std::os::unix::fs::symlink(home.join(".ssh"), home.join("keys")).expect("symlink");
    let kept = |dir: &str| handed_to_jail(&home.join(dir), Some(&home));
    assert!(kept("work"));
    assert!(kept("work/build"));
    for refused in [
        "",
        ".config",
        ".config/magi",
        ".config/magi/plugin",
        ".ssh",
        ".local/share",
        "keys",
    ] {
        assert!(!kept(refused), "{refused:?} was handed over");
    }
    assert!(!handed_to_jail(Path::new("/"), Some(&home)));
    assert!(
        !handed_to_jail(Path::new("/"), None),
        "the root, with no home"
    );
}

#[test]
fn the_network_stays_open_whatever_the_grants_say() {
    use magi_proto::permit::{Grant, Scope};
    for grants in [
        vec![],
        vec![Grant {
            verb: "reach".to_owned(),
            scope: Scope::Directory {
                path: "example.test".to_owned(),
            },
        }],
    ] {
        let dir = Scratch::new("magi-ops", "network-floor");
        let ops = Real::gated(
            dir.to_path_buf(),
            crate::permit::Ledger::with(grants),
            std::sync::Arc::new(crate::approve::AllowAll),
        )
        .isolating(true);
        let profile: serde_json::Value =
            serde_json::from_str(&ops.jail().expect("profile")).expect("json");
        assert_eq!(profile["reach"], true);
        if let Some(tmp) = ops.shared_tmp() {
            let _ = std::fs::remove_dir_all(tmp);
        }
    }
}

#[test]
fn the_jail_is_handed_this_projects_spill_directory_and_nothing_above_it() {
    let (ops, dir) = rooted("spills");
    let ops = ops.isolating(true);
    let profile: serde_json::Value =
        serde_json::from_str(&ops.jail().expect("a profile")).expect("json");
    let spills = crate::bound::spill_dir(&dir).expect("made");
    let write = profile["write"].as_array().expect("a write list");
    assert!(write.contains(&serde_json::json!(spills)), "{profile}");
    let above = serde_json::json!(spills.parent().expect("the user's"));
    assert!(!write.contains(&above), "{profile}");
    let _ = std::fs::remove_dir(&spills);
    if let Some(tmp) = ops.shared_tmp() {
        let _ = std::fs::remove_dir_all(tmp);
    }
}

#[test]
fn the_jail_profile_is_built_from_the_grants() {
    use magi_proto::permit::{Grant, Scope};
    let ledger = crate::permit::Ledger::with(vec![
        Grant {
            verb: "write".to_owned(),
            scope: Scope::Directory {
                path: "/w/build".to_owned(),
            },
        },
        Grant {
            verb: "reach".to_owned(),
            scope: Scope::Anything,
        },
    ]);
    let ops = Real::gated(
        std::path::PathBuf::from("/w"),
        ledger,
        std::sync::Arc::new(crate::approve::AllowAll),
    )
    .isolating(true);
    let json = ops.jail().expect("a profile when isolation is on");
    assert!(
        json.contains("/w/build"),
        "the write grant is in the profile: {json}"
    );
    assert!(
        json.contains("\"reach\":true"),
        "the reach grant opens the network: {json}"
    );
    assert!(json.contains("\"tmp\":"), "a shared /tmp is named: {json}");
    // The profile made a runtime directory; remove the one this test created.
    if let Some(tmp) = ops.shared_tmp() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// The same, with the wall on: `magi.confine` is where that rule lives now.
fn walled(name: &str) -> (Real, Scratch) {
    let dir = Scratch::new("magi-wall", name);
    (Real::confined(dir.to_path_buf()), dir)
}

#[test]
fn a_symlink_is_seen_through_before_gating_and_never_written_through() {
    let (ops, dir) = rooted("links");
    std::fs::create_dir_all(dir.join(".git/hooks")).expect("mkdir");
    std::os::unix::fs::symlink(dir.join(".git/hooks"), dir.join("h")).expect("symlink");
    std::os::unix::fs::symlink(dir.join(".git/config"), dir.join("c")).expect("symlink");
    let real = |path: &str| ops.resolved(Path::new(path));
    assert_eq!(real("h/post-checkout"), real(".git/hooks/post-checkout"));
    assert_eq!(real("c"), real(".git/config"), "a link to nothing yet");

    let refused = ops.write(Path::new("c"), "[core]").expect_err("refused");
    assert!(refused.contains("symbolic link"), "{refused}");
    assert!(
        !dir.join(".git/config").exists(),
        "the write went through the link"
    );

    let walled = Real::confined(dir.join("inner"));
    std::fs::create_dir_all(dir.join("inner")).expect("mkdir");
    std::os::unix::fs::symlink("/etc", dir.join("inner/x")).expect("symlink");
    assert!(
        walled.read(Path::new("x/hostname")).is_err(),
        "out through a link"
    );
}

#[test]
fn a_write_then_a_read_round_trips() {
    let (ops, _dir) = rooted("roundtrip");
    ops.write(Path::new("a.txt"), "hello").expect("write");
    assert_eq!(ops.read(Path::new("a.txt")).expect("read"), "hello");
}

#[test]
fn a_write_creates_parent_directories() {
    let (ops, dir) = rooted("parents");
    ops.write(Path::new("deep/nested/a.txt"), "x")
        .expect("write");
    assert!(dir.join("deep/nested/a.txt").exists());
}

#[test]
fn a_confined_path_that_escapes_the_root_is_refused() {
    let (ops, _dir) = walled("escape");
    let error = ops
        .read(Path::new("../../etc/passwd"))
        .expect_err("must refuse");
    assert!(error.contains("outside"), "{error}");
}

#[test]
fn an_escape_hidden_behind_a_descent_is_still_refused() {
    // `a/../../etc` has no leading `..` and still escapes, so the check happens after normalising.
    let (ops, _dir) = rooted("hidden");
    assert!(ops.read(Path::new("a/../../etc/passwd")).is_err());
}

#[test]
fn a_confined_absolute_path_outside_the_root_is_refused() {
    let (ops, _dir) = walled("absolute");
    assert!(ops.read(Path::new("/etc/passwd")).is_err());
}

#[test]
fn reading_a_missing_file_names_it() {
    let (ops, _dir) = rooted("missing");
    let error = ops.read(Path::new("nope.txt")).expect_err("must fail");
    assert!(error.contains("nope.txt"), "{error}");
}
