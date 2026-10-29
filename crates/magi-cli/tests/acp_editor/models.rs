use super::*;
use serde_json::json;

pub fn configure(root: &Path, mind: &Mind) {
    let cards: Vec<_> = ["one", "two", "unavailable"]
        .into_iter()
        .map(|name| {
            json!({
                "id":format!("fake/{name}"), "provider":"fake", "name":name,
                "api":"anthropic-messages", "context_window":200000, "max_output":4096,
                "reasons":true, "ready":name != "unavailable"
            })
        })
        .collect();
    let models = json!({"ok":true,"n":cards.len(),"result":cards});
    let quote = |path: &Path| format!("'{}'", path.to_str().expect("path").replace('\'', "'\\''"));
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\nmodels) printf '%s\\n' '{models}' ;;\nask) exec {} \"$@\" ;;\n*) exec {} \"$@\" ;;\nesac\n",
        quote(mind.program()),
        quote(&selected("MELCHIOR_TEST_BINARY"))
    );
    let path = root.join("bin/melchior");
    std::fs::write(&path, script).expect("three synthetic model choices");
    magi_model::scratch::executable_ready(&path);
}

async fn terminal_marker(
    root: &Path,
    evidence: &Path,
    input: &mut tokio::process::ChildStdin,
    name: &str,
    seen: &mut usize,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let notice = b"Press ENTER or type command to continue";
    let mut acknowledged = 0;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if root.join(name).is_file() {
                return Ok::<_, anyhow::Error>(());
            }
            let bytes = match std::fs::read(evidence.join("editor.stdout")) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.into()),
            };
            let fresh = bytes
                .get(*seen..)
                .context("terminal output cannot shrink")?;
            if fresh.windows(notice.len()).any(|window| window == notice) {
                ensure!(
                    acknowledged < 2,
                    "bounded terminal notification acknowledgements"
                );
                *seen = bytes.len();
                input.write_all(b"\r").await?;
                acknowledged += 1;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("{name} terminal deadline"))?
}

async fn picker(
    root: &Path,
    evidence: &Path,
    input: &mut tokio::process::ChildStdin,
    name: &str,
    seen: &mut usize,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    terminal_marker(root, evidence, input, &format!("{name}.ready"), seen).await?;
    let index = std::fs::read_to_string(root.join(format!("{name}.ready")))?
        .trim()
        .parse::<usize>()?;
    ensure!((1..=3).contains(&index), "bounded model picker choice");
    let output = evidence.join("editor.stdout");
    let offset = std::fs::metadata(&output)?.len() as usize;
    std::fs::write(root.join(format!("{name}.armed")), b"ready")?;
    let bytes = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let bytes = std::fs::read(&output)?;
            let fresh = bytes
                .get(offset..)
                .context("terminal output cannot shrink")?;
            let rendered = String::from_utf8_lossy(fresh);
            if rendered.contains("Type number and <Enter>") {
                ensure!(
                    rendered.contains("Select model to change:"),
                    "actual model picker prompt"
                );
                ensure!(
                    rendered.contains("fake/two") && rendered.contains("fake/unavailable"),
                    "actual model options rendered"
                );
                return Ok::<_, anyhow::Error>(fresh.to_vec());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("rendered model picker deadline")??;
    *seen = offset + bytes.len();
    std::fs::write(root.join(format!("{name}.ansi")), bytes)?;
    input.write_all(format!("{index}\r").as_bytes()).await?;
    Ok(())
}

pub async fn steps(
    root: &Path,
    evidence: &Path,
    mind: &Mind,
    input: &mut tokio::process::ChildStdin,
) -> Result<()> {
    let mut seen = 0;
    picker(root, evidence, input, "model-busy-picker", &mut seen).await?;
    picker(root, evidence, input, "model-select-picker", &mut seen).await?;
    terminal_marker(root, evidence, input, "model-selected-ready", &mut seen).await?;
    ensure!(mind.asked() == 1, "selection does not invoke a provider");
    ensure!(
        !mind.on_path().join("release.0").exists(),
        "busy prompt was cancelled without release"
    );
    std::fs::write(root.join("model-selected-checked"), b"verified")?;
    terminal_marker(root, evidence, input, "switched-prompt-ready", &mut seen).await?;
    ensure!(mind.asked() == 2, "one prompt with selected model");
    let asks = mind.asks();
    ensure!(
        serde_json::from_str::<Value>(&asks[0])?["model"] == "fake/one",
        "initial native model"
    );
    ensure!(
        serde_json::from_str::<Value>(&asks[1])?["model"] == "fake/two",
        "changed model used by native provider"
    );
    mind.release(1);
    picker(root, evidence, input, "model-refuse-picker", &mut seen).await?;
    terminal_marker(root, evidence, input, "refusal-prompt-ready", &mut seen).await?;
    ensure!(mind.asked() == 3, "no hidden call during refused selection");
    ensure!(
        serde_json::from_str::<Value>(&mind.asks()[2])?["model"] == "fake/two",
        "refused selection retains model"
    );
    ensure!(
        !root.join("d/magi/chosen.json").exists(),
        "session-local selection does not persist a default"
    );
    ensure!(
        !root.join("d/magi/chosen.lock").exists(),
        "session-local selection does not acquire saved-default lock"
    );
    mind.release(2);
    Ok(())
}

pub fn verify(receipt: &Value) {
    assert_eq!(receipt["completions"], 3);
    assert_eq!(receipt["selected_model"], "fake/two");
    assert_eq!(receipt["busy_change_refused"], true);
    assert_eq!(receipt["unavailable_change_refused"], true);
}

pub const SCREENS: &[(&str, &str)] = &[
    ("model-selected-screen.txt", "fake/two"),
    ("model-recovered-screen.txt", "part-2-done"),
];
