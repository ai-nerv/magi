#![allow(clippy::expect_used, clippy::indexing_slicing)]

#[path = "acp_editor/loading.rs"]
mod loading;
#[path = "acp_editor/models.rs"]
mod models;
#[path = "acp_editor/permissions.rs"]
mod permissions;
#[path = "acp_editor/supervision.rs"]
mod supervision;
#[path = "acp_editor/tool_cancel.rs"]
mod tool_cancel;

use anyhow::{Context, Result, bail, ensure};
use magi_model::scratch::Scratch;
use magi_testkit::Mind;
use serde_json::Value;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const CLIENT: &str = "f0bef086b3ed6121d9a18a5a820d071c69f2f0f5";

#[derive(Clone, Copy)]
enum Scenario {
    Streaming,
    Permissions,
    Models,
    ToolCancel,
}

impl Scenario {
    fn name(self, terminal: bool) -> &'static str {
        match (self, terminal) {
            (Self::Streaming, false) => "buffer",
            (Self::Streaming, true) => "terminal",
            (Self::Permissions, false) => "permission-buffer",
            (Self::Permissions, true) => "permission-terminal",
            (Self::Models, false) => "model-buffer",
            (Self::Models, true) => "model-terminal",
            (Self::ToolCancel, false) => "tool-cancel-buffer",
            (Self::ToolCancel, true) => "tool-cancel-terminal",
        }
    }

    fn screens(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Streaming => &[
                ("streaming-screen.txt", "part-0"),
                ("completed-screen.txt", "part-1-done"),
                ("cancelled-screen.txt", "Generation stopped"),
                ("recovery-screen.txt", "part-3-done"),
            ],
            Self::Permissions => permissions::SCREENS,
            Self::Models => models::SCREENS,
            Self::ToolCancel => tool_cancel::SCREENS,
        }
    }
}

fn selected(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).expect(name));
    assert!(path.is_absolute() && path.is_file(), "{name}: {path:?}");
    std::fs::canonicalize(path).expect("selected executable")
}

fn client() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("MAGI_TEST_EDITOR_CLIENT").expect("client source"));
    assert!(root.is_absolute());
    let revision = std::process::Command::new("git")
        .args(["-C", root.to_str().expect("path"), "rev-parse", "HEAD"])
        .output()
        .expect("client revision");
    assert!(revision.status.success());
    assert_eq!(String::from_utf8_lossy(&revision.stdout).trim(), CLIENT);
    let status = std::process::Command::new("git")
        .args([
            "-C",
            root.to_str().expect("path"),
            "status",
            "--porcelain",
            "--untracked-files=all",
        ])
        .output()
        .expect("client source identity");
    assert!(
        status.status.success() && status.stdout.is_empty(),
        "client checkout must be unchanged"
    );
    root
}

fn utility(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("tools PATH"))
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
        .and_then(|path| std::fs::canonicalize(path).ok())
        .expect(name)
}

fn configure(root: &Path, mind: &Mind) -> std::ffi::OsString {
    for directory in [
        "bin", "r", "c/magi", "c/casper", "d", "s", "h", "cache", "a/.git",
    ] {
        std::fs::create_dir_all(root.join(directory)).expect("fixture directories");
    }
    std::fs::set_permissions(root.join("r"), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    for (name, variable) in [
        ("casper", "CASPER_TEST_BINARY"),
        ("balthasar", "BALTHASAR_TEST_BINARY"),
    ] {
        symlink(selected(variable), root.join("bin").join(name)).expect("selected sibling");
    }
    let router = selected("MELCHIOR_TEST_BINARY");
    let quote = |path: &Path| format!("'{}'", path.to_str().expect("path").replace('\'', "'\\''"));
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in models|ask) exec {} \"$@\" ;; *) exec {} \"$@\" ;; esac\n",
        quote(mind.program()),
        quote(&router)
    );
    let path = root.join("bin/melchior");
    std::fs::write(&path, script).expect("synthetic model boundary");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("executable router");
    magi_model::scratch::executable_ready(&path);
    std::fs::copy(
        selected("CASPER_TEST_TOOLS"),
        root.join("c/casper/tools.lua"),
    )
    .expect("selected tools");
    std::fs::write(
        root.join("c/magi/init.lua"),
        "magi.model = 'fake/one'\nmagi.helpers = { memory = false }\nmagi.may_spawn = false\n",
    )
    .expect("synthetic-only catalog");
    std::fs::write(root.join("smoke.lua"), include_str!("acp_editor/smoke.lua"))
        .expect("editor fixture");
    for name in [
        "sh", "head", "cat", "dirname", "mkdir", "rm", "rmdir", "touch", "sleep", "bwrap",
    ] {
        symlink(utility(name), root.join("bin").join(name)).expect("fixture tool");
    }
    std::env::join_paths([
        root.join("bin"),
        utility("head")
            .parent()
            .expect("selected core utilities")
            .to_owned(),
        utility("sh").parent().expect("selected shell").to_owned(),
    ])
    .expect("native sandbox tool PATH")
}

async fn marker(root: &Path, name: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            if root.join(name).is_file() {
                return Ok(());
            }
            if let Ok(why) = std::fs::read_to_string(root.join("editor-error.txt")) {
                bail!("editor: {why}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("{name} deadline"))?
}

async fn reaped(root: &Path) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let live = std::fs::read_dir("/proc")
                .expect("processes")
                .flatten()
                .any(|entry| {
                    std::fs::read_link(entry.path().join("cwd"))
                        .is_ok_and(|cwd| cwd.starts_with(root))
                });
            if !live {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("zero owned editor/adapter/host/provider processes")
}

#[tokio::test]
#[ignore = "requires explicit pinned editor inputs; run oslo make test-acp-editor-smoke"]
async fn neovim_agentic_streams_native_turns_into_its_real_chat_buffer() {
    smoke(false, Scenario::Streaming).await;
}

#[tokio::test]
#[ignore = "requires explicit pinned editor inputs; run oslo make test-acp-editor-smoke"]
async fn neovim_agentic_renders_streaming_and_completed_turns_in_a_terminal() {
    smoke(true, Scenario::Streaming).await;
}

#[tokio::test]
#[ignore = "requires explicit pinned editor inputs; run oslo make test-acp-editor-smoke"]
async fn neovim_agentic_permission_choices_control_native_tool_effects() {
    smoke(false, Scenario::Permissions).await;
}

#[tokio::test]
#[ignore = "requires explicit pinned editor inputs; run oslo make test-acp-editor-smoke"]
async fn neovim_agentic_renders_permissions_and_selects_them_with_real_keymaps() {
    smoke(true, Scenario::Permissions).await;
}

#[tokio::test]
#[ignore = "requires explicit pinned editor inputs; run oslo make test-acp-editor-smoke"]
async fn neovim_agentic_model_picker_preserves_native_selection_and_refusals() {
    smoke(true, Scenario::Models).await;
}

#[tokio::test]
#[ignore = "requires explicit pinned editor inputs; run oslo make test-acp-editor-smoke"]
async fn neovim_agentic_cancels_a_running_native_tool_before_recovery() {
    smoke(true, Scenario::ToolCancel).await;
}

async fn smoke(terminal: bool, scenario: Scenario) {
    let binary = std::fs::canonicalize(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
        .expect("selected Magi executable");
    let editor = selected("MAGI_TEST_EDITOR");
    let client = client();
    let evidence =
        PathBuf::from(std::env::var_os("MAGI_EDITOR_EVIDENCE").expect("evidence directory"))
            .join(scenario.name(terminal));
    assert!(evidence.is_absolute());
    std::fs::create_dir_all(evidence.parent().expect("evidence parent")).expect("evidence root");
    std::fs::create_dir(&evidence).expect("fresh evidence directory");
    let root = Scratch::new("acp-editor", "native").settling();
    let name = format!("acp-editor-{}", scenario.name(terminal));
    let mind = match scenario {
        Scenario::Streaming | Scenario::Models => Mind::controlled(&name),
        Scenario::Permissions => permissions::provider(&root, &name),
        Scenario::ToolCancel => tool_cancel::provider(&root, &name),
    };
    let path = configure(&root, &mind);
    if matches!(scenario, Scenario::Models) {
        models::configure(&root, &mind);
    }
    std::fs::write(
        root.join("scenario.lua"),
        match scenario {
            Scenario::Streaming => include_str!("acp_editor/streaming.lua"),
            Scenario::Permissions => include_str!("acp_editor/permissions.lua"),
            Scenario::Models => include_str!("acp_editor/models.lua"),
            Scenario::ToolCancel => include_str!("acp_editor/tool_cancel.lua"),
        },
    )
    .expect("selected editor scenario");
    std::fs::write(root.join("a/permit-note"), "editor-allowed-read")
        .expect("disposable permitted file");
    let mut command = if terminal {
        let mut command = tokio::process::Command::new(selected("MAGI_TEST_PTY"));
        command
            .args([
                "--return",
                "--quiet",
                "--flush",
                "--echo",
                "never",
                "/dev/null",
                "--",
            ])
            .arg(&editor);
        command
    } else {
        let mut command = tokio::process::Command::new(&editor);
        command.arg("--headless");
        command
    };
    let mut child = command
        .args([
            "-u",
            "NONE",
            "-i",
            "NONE",
            "--noplugin",
            if terminal { "-S" } else { "-l" },
        ])
        .arg(root.join("smoke.lua"))
        .env_clear()
        .env("HOME", root.join("h"))
        .env("PATH", path)
        .env("XDG_CONFIG_HOME", root.join("c"))
        .env("XDG_DATA_HOME", root.join("d"))
        .env("XDG_STATE_HOME", root.join("s"))
        .env("XDG_RUNTIME_DIR", root.join("r"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("MAGI_TEST_EDITOR_CLIENT", &client)
        .env("MAGI_TEST_BINARY", &binary)
        .env("MAGI_EDITOR_TERMINAL", if terminal { "1" } else { "0" })
        .env("TERM", "xterm-256color")
        .env("SHELL", root.join("bin/sh"))
        .current_dir(root.join("a"))
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated editor");
    let mut input = child.stdin.take().expect("terminal input");
    let result = supervision::run(&mut child, &evidence, Duration::from_secs(60), async {
        if matches!(scenario, Scenario::Permissions) {
            return permissions::steps(&root, &mind).await;
        }
        if matches!(scenario, Scenario::Models) {
            return models::steps(&root, &evidence, &mind, &mut input).await;
        }
        if matches!(scenario, Scenario::ToolCancel) {
            return tool_cancel::steps(&root, &mind).await;
        }
        marker(&root, "stream-ready").await?;
        ensure!(mind.asked() == 1, "exactly one first native prompt");
        mind.release(0);
        marker(&root, "second-ready").await?;
        ensure!(mind.asked() == 2, "exactly two native prompts");
        ensure!(
            mind.asks()[1].contains("editor-stream-first"),
            "native history survives second editor prompt"
        );
        mind.release(1);
        marker(&root, "cancel-ready").await?;
        ensure!(mind.asked() == 3, "exactly one cancellable third prompt");
        std::fs::write(root.join("cancel-go"), b"cancel")?;
        marker(&root, "recovery-ready").await?;
        ensure!(mind.asked() == 4, "exactly one post-cancel prompt");
        ensure!(
            !mind.overlapped(),
            "cancelled provider settled before recovery"
        );
        ensure!(
            !mind.on_path().join("release.2").exists(),
            "cancelled provider was never released"
        );
        ensure!(
            mind.asks()[3].contains("editor-stream-first"),
            "native history survives cancellation"
        );
        mind.release(3);
        Ok(())
    })
    .await;
    let preserved: Vec<_> = [
        "adapter-identity.json",
        "streaming.txt",
        "editor-receipt.json",
        "editor-error.txt",
        "editor-failure.json",
        "model-busy-picker.ansi",
        "model-select-picker.ansi",
        "model-refuse-picker.ansi",
        "tool-process-receipt.json",
    ]
    .into_iter()
    .chain(scenario.screens().iter().map(|(file, _)| *file))
    .map(|file| supervision::preserve(&root, &evidence, file))
    .collect();
    let cleanup = reaped(&root).await;
    std::fs::write(
        evidence.join("supervision.txt"),
        format!("result: {result:?}\ncleanup: {cleanup:?}\npreservation: {preserved:?}\nmodel_calls: {}\n", mind.asked()),
    )
    .expect("supervision evidence");
    cleanup.expect("owned cleanup");
    for result in preserved {
        result.expect("bounded artifact preservation");
    }
    assert!(
        result.expect("supervised editor").success(),
        "inspect {}",
        evidence.display()
    );
    let receipt: Value = serde_json::from_slice(
        &std::fs::read(evidence.join("editor-receipt.json")).expect("receipt"),
    )
    .expect("JSON receipt");
    assert_eq!(receipt["client_commit"], CLIENT);
    assert_eq!(
        receipt["adapter"]["executable"],
        binary.to_str().expect("binary path")
    );
    assert!(
        receipt["adapter"]["pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0)
    );
    match scenario {
        Scenario::Streaming => {
            assert_eq!(receipt["completions"], 4);
            assert_eq!(receipt["cancelled_turns"], 1);
            assert_eq!(receipt["post_cancel_recovery"], true);
        }
        Scenario::Permissions => permissions::verify(&receipt),
        Scenario::Models => models::verify(&receipt),
        Scenario::ToolCancel => tool_cancel::verify(&receipt),
    }
    assert_eq!(receipt["visual_acceptance"], false);
    assert_eq!(receipt["persistent_load_acceptance"], false);
    assert_eq!(receipt["terminal_rendered"], terminal);
    if terminal {
        for (file, text) in scenario.screens() {
            let screen = std::fs::read_to_string(evidence.join(file)).expect("rendered screen");
            assert!(screen.contains(*text), "{file}: {screen}");
            assert_eq!(screen.lines().count(), 32);
        }
        assert!(
            std::fs::metadata(evidence.join("editor.stdout"))
                .expect("PTY recording")
                .len()
                > 0
        );
    }
    assert!(
        receipt["session"]
            .as_str()
            .expect("session")
            .starts_with("magi-v1-")
    );
    assert!(!root.join("d/magi/chosen.json").exists());
    for file in ["editor.stdout", "editor.stderr", "editor-receipt.json"] {
        assert!(
            std::fs::metadata(evidence.join(file))
                .expect("evidence metadata")
                .len()
                <= 1_048_576
        );
    }
}
