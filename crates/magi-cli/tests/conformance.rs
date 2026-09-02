//! Every peer, against the same definition of what a peer must do.
//!
//! Including one that shares no code with magi at all: `tests/fixtures/echo.c` was written
//! from the documented wire format, in another language, with its own hand-rolled CBOR. That
//! is the only check that says the protocol is written down rather than merely implemented —
//! two peers built on the same Rust codec cannot disagree with the host, so they cannot show
//! it. If the C peer stops passing, the documentation was not enough.

use magi_testkit::conformance::{Subject, check};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("magi-conf-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn assert_conforms(subject: &Subject<'_>) {
    let findings = check(subject);
    assert!(
        findings.is_empty(),
        "{} does not conform:\n{}",
        subject.command,
        findings
            .iter()
            .map(|f| format!("  {} — {}", f.rule, f.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_shell_peer_conforms() {
    let dir = scratch("shell");
    assert_conforms(&Subject {
        command: env!("CARGO_BIN_EXE_magi"),
        args: &["ext".into(), "shell".into()],
        dir: &dir,
        call: ("shell", serde_json::json!({ "command": "echo hi" })),
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_lua_peer_conforms() {
    let dir = scratch("lua");
    let file = dir.join("peer.lua");
    std::fs::write(
        &file,
        r#"
magi.tool("greet", {
  description = "Say hello.",
  parameters = { type = "object", properties = { who = { type = "string" } } },
  run = function(args) return "hello " .. tostring(args.who) end,
})
"#,
    )
    .expect("write");
    assert_conforms(&Subject {
        command: env!("CARGO_BIN_EXE_magi"),
        args: &["ext".into(), "lua".into(), file.display().to_string()],
        dir: &dir,
        call: ("greet", serde_json::json!({ "who": "world" })),
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_shares_no_code_with_magi_conforms() {
    // The one that matters. Written from the documentation, in C, with its own CBOR encoder.
    let dir = scratch("foreign");
    let Some(binary) = build_c_peer(&dir) else {
        // Skipped rather than failed: a machine with no C compiler cannot answer this
        // question, and pretending otherwise would make the suite lie about what it checked.
        eprintln!("no C compiler; the foreign-peer check did not run");
        return;
    };
    assert_conforms(&Subject {
        command: &binary.display().to_string(),
        args: &[],
        dir: &dir,
        call: (
            "echo",
            serde_json::json!({ "text": "from another language" }),
        ),
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_declares_nothing_is_reported() {
    // The suite has to fail things, or passing means nothing. `true` exits at once, having
    // declared nothing and answered nothing.
    let dir = scratch("silent");
    let findings = check(&Subject {
        command: "true",
        args: &[],
        dir: &dir,
        call: ("anything", serde_json::json!({})),
    });
    assert!(
        findings
            .iter()
            .any(|f| f.rule.contains("declare at least one tool")),
        "{findings:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_never_answers_is_reported() {
    // `cat` echoes its input back, which is not a declaration and not a result.
    let dir = scratch("mute");
    let findings = check(&Subject {
        command: "cat",
        args: &[],
        dir: &dir,
        call: ("anything", serde_json::json!({})),
    });
    assert!(!findings.is_empty(), "a peer that says nothing must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Compile the C peer, or `None` if this machine has no compiler.
fn build_c_peer(dir: &Path) -> Option<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/echo.c");
    let binary = dir.join("echo");
    for compiler in ["cc", "gcc", "clang"] {
        let built = std::process::Command::new(compiler)
            .arg("-o")
            .arg(&binary)
            .arg(&source)
            .output();
        if built.is_ok_and(|o| o.status.success()) {
            return Some(binary);
        }
    }
    None
}
