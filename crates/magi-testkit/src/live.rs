//! Prerequisites for optional standalone and required family integration tests.

/// The family's selected executable, or Cargo's executable for a standalone suite.
#[must_use]
pub fn binary(cargo: &str) -> std::ffi::OsString {
    std::env::var_os("MAGI_TEST_BINARY").unwrap_or_else(|| cargo.into())
}

/// Report an unavailable prerequisite, failing when the family lane requires it.
///
/// # Panics
/// When `MAGI_REQUIRE_LIVE=1`.
pub fn unavailable(reason: &str) {
    assert!(
        !std::env::var("MAGI_REQUIRE_LIVE").is_ok_and(|value| value == "1"),
        "required family prerequisite unavailable: {reason}"
    );
    eprintln!("skipping: {reason}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn binary_selection_is_process_local() {
        if let Ok(expected) = std::env::var("MAGI_TEST_BINARY_EXPECTED") {
            assert_eq!(
                super::binary("cargo-binary"),
                std::ffi::OsString::from(expected)
            );
            return;
        }
        for selected in [None, Some("/selected/magi")] {
            let mut child =
                std::process::Command::new(std::env::current_exe().expect("test binary"));
            child
                .args(["--exact", "live::tests::binary_selection_is_process_local"])
                .env_remove("MAGI_TEST_BINARY")
                .env(
                    "MAGI_TEST_BINARY_EXPECTED",
                    selected.unwrap_or("cargo-binary"),
                );
            if let Some(selected) = selected {
                child.env("MAGI_TEST_BINARY", selected);
            }
            assert!(child.status().expect("binary selection fixture").success());
        }
    }

    #[test]
    fn prerequisite_policy_is_process_local() {
        if std::env::var_os("MAGI_TEST_PREREQUISITE_CHILD").is_some() {
            super::unavailable("synthetic missing sibling");
            return;
        }
        for (required, succeeds) in [("0", true), ("1", false)] {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "live::tests::prerequisite_policy_is_process_local",
                ])
                .env("MAGI_TEST_PREREQUISITE_CHILD", "1")
                .env("MAGI_REQUIRE_LIVE", required)
                .output()
                .expect("run prerequisite fixture");
            assert_eq!(output.status.success(), succeeds);
            if !succeeds {
                assert!(
                    String::from_utf8_lossy(&output.stdout)
                        .contains("required family prerequisite unavailable")
                );
            }
        }
    }
}
