use super::*;

async fn stage(
    root: &Path,
    evidence: &Path,
    binary: &Path,
    path: &std::ffi::OsStr,
    mind: &Mind,
    loading: bool,
) -> Result<Value> {
    std::fs::create_dir(evidence)?;
    let mut child = tokio::process::Command::new(selected("MAGI_TEST_PTY"))
        .args([
            "--return",
            "--quiet",
            "--flush",
            "--echo",
            "never",
            "/dev/null",
            "--",
        ])
        .arg(selected("MAGI_TEST_EDITOR"))
        .args(["-u", "NONE", "-i", "NONE", "--noplugin", "-S"])
        .arg(root.join("loading.lua"))
        .env_clear()
        .env("HOME", root.join("h"))
        .env("PATH", path)
        .env("XDG_CONFIG_HOME", root.join("c"))
        .env("XDG_DATA_HOME", root.join("d"))
        .env("XDG_STATE_HOME", root.join("s"))
        .env("XDG_RUNTIME_DIR", root.join("r"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("MAGI_TEST_EDITOR_CLIENT", client())
        .env("MAGI_TEST_BINARY", binary)
        .env("MAGI_EDITOR_LOAD", if loading { "1" } else { "0" })
        .env("TERM", "xterm-256color")
        .env("SHELL", root.join("bin/sh"))
        .current_dir(root.join("a"))
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let result = supervision::run(&mut child, evidence, Duration::from_secs(60), async {
        if loading {
            marker(root, "load-replayed").await?;
            ensure!(mind.asked() == 1, "editor replay makes no provider call");
            std::fs::write(root.join("load-replay-checked"), b"verified")?;
            marker(root, "load-next-ready").await?;
            ensure!(mind.asked() == 2, "one continued provider call");
            let asks = mind.asks();
            let continued = asks.get(1).context("continued native request")?;
            for text in [
                "editor-persistent-first",
                "part-0-done",
                "editor-persistent-next",
            ] {
                ensure!(
                    continued.contains(text),
                    "native continuation includes {text}"
                );
            }
            mind.release(1);
        } else {
            marker(root, "load-seed-ready").await?;
            ensure!(mind.asked() == 1, "one seed provider call");
            mind.release(0);
        }
        Ok(())
    })
    .await;
    let cleanup = reaped(root).await;
    for name in [
        "load-editor-receipt.json",
        "editor-error.txt",
        "saved-session.txt",
        "load-seed-screen.txt",
        "load-replay-screen.txt",
        "load-continued-screen.txt",
        "load-replay-buffer.txt",
    ] {
        supervision::preserve(root, evidence, name)?;
    }
    std::fs::write(
        evidence.join("supervision.txt"),
        format!(
            "result: {result:?}\ncleanup: {cleanup:?}\nmodel_calls: {}\n",
            mind.asked()
        ),
    )?;
    cleanup?;
    ensure!(
        result?.success(),
        "editor stage failed: {}",
        evidence.display()
    );
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(evidence.join("load-editor-receipt.json"))?)?;
    ensure!(
        receipt["adapter"]["executable"] == binary.to_str().context("binary path")?,
        "selected adapter"
    );
    ensure!(
        receipt["adapter"]["pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0),
        "actual adapter PID"
    );
    ensure!(receipt["client_commit"] == CLIENT, "pinned client");
    ensure!(
        receipt["restored"] == loading && receipt["completions"] == 1,
        "exact native completion"
    );
    ensure!(
        receipt["terminal_rendered"] == true,
        "actual terminal rendering"
    );
    for (name, text) in if loading {
        &[
            ("load-replay-screen.txt", "part-0-done"),
            ("load-continued-screen.txt", "part-1-done"),
        ][..]
    } else {
        &[("load-seed-screen.txt", "part-0-done")][..]
    } {
        let screen = std::fs::read_to_string(evidence.join(name))?;
        ensure!(
            screen.lines().count() == 32 && screen.contains(text),
            "rendered {name}"
        );
    }
    Ok(receipt)
}

#[tokio::test]
#[ignore = "requires explicit pinned editor inputs; run oslo make test-acp-editor-smoke"]
async fn neovim_agentic_loads_in_a_fresh_editor_and_continues_native_history() {
    let binary = std::fs::canonicalize(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
        .expect("selected Magi");
    let evidence = PathBuf::from(std::env::var_os("MAGI_EDITOR_EVIDENCE").expect("evidence"))
        .join("loading-terminal");
    assert!(evidence.is_absolute());
    std::fs::create_dir_all(evidence.parent().expect("parent")).expect("evidence parent");
    std::fs::create_dir(&evidence).expect("fresh evidence");
    let root = Scratch::new("acp-editor", "loading").settling();
    let mind = Mind::controlled("acp-editor-persistent-loading");
    let path = configure(&root, &mind);
    std::fs::write(root.join("loading.lua"), include_str!("loading.lua")).expect("editor script");
    let seed = stage(&root, &evidence.join("seed"), &binary, &path, &mind, false)
        .await
        .expect("seed editor");
    let saved = std::fs::read_to_string(root.join("saved-session.txt")).expect("durable identity");
    assert_eq!(seed["session"], saved.trim());
    std::fs::remove_file(root.join("load-editor-receipt.json")).expect("fresh load receipt");
    let loaded = stage(&root, &evidence.join("load"), &binary, &path, &mind, true)
        .await
        .expect("restored editor");
    assert_eq!(loaded["session"], seed["session"]);
    assert_ne!(loaded["adapter"]["pid"], seed["adapter"]["pid"]);
    assert_eq!(mind.asked(), 2);
    assert!(!root.join("d/magi/chosen.json").exists());
}
