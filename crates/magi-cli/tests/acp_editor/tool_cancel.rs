use super::*;
use magi_testkit::mind::{call_lines, stop_line, text_line};
use serde_json::json;

#[path = "../support/tool_process.rs"]
mod tool_process;

pub fn provider(root: &Path, name: &str) -> Mind {
    std::fs::create_dir_all(root.join("a")).expect("tool workspace");
    let barrier = root.join("a/tool-barrier");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &barrier,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("tool barrier FIFO");
    let command = format!(
        "printf started > '{}'; cat '{}'; printf completed > '{}'",
        root.join("a/tool-started").display(),
        barrier.display(),
        root.join("a/tool-completed").display()
    );
    let shell = call_lines(
        "held-shell",
        "shell",
        &json!({"command":command}).to_string(),
    );
    let recovery = [text_line("editor-tool-recovered"), stop_line()];
    let shell: Vec<_> = shell.iter().map(String::as_str).collect();
    let recovery: Vec<_> = recovery.iter().map(String::as_str).collect();
    Mind::turns(name, &[&shell, &recovery])
}

pub async fn steps(root: &Path, mind: &Mind) -> Result<()> {
    marker(root, "tool-permission-ready").await?;
    ensure!(mind.asked() == 1, "one prompt pending approval");
    ensure!(
        !root.join("a/tool-started").exists(),
        "no shell execution before approval"
    );
    std::fs::write(root.join("tool-permission-go"), b"allow once")?;
    let readers = tool_process::started(root).await?;
    ensure!(mind.asked() == 1, "no followup while tool is blocked");
    ensure!(
        !root.join("a/tool-completed").exists(),
        "held shell has no final effect"
    );
    std::fs::write(root.join("tool-cancel-go"), b"observed native reader")?;
    marker(root, "tool-cancelled").await?;
    tool_process::reaped(&readers).await?;
    ensure!(
        mind.asked() == 1,
        "cancel does not start a provider followup"
    );
    ensure!(
        !root.join("a/tool-completed").exists(),
        "cancelled shell has no final effect"
    );
    std::fs::write(root.join("tool-recovery-go"), b"reader reaped")?;
    marker(root, "tool-recovered").await?;
    ensure!(mind.asked() == 2, "exactly one explicit recovery prompt");
    ensure!(!mind.overlapped(), "no overlapping provider calls");
    ensure!(
        mind.asks()[1].contains("editor-tool-recovery"),
        "recovery reaches native provider"
    );
    ensure!(
        !root.join("a/tool-completed").exists(),
        "no delayed tool effect"
    );
    std::fs::write(
        root.join("tool-process-receipt.json"),
        serde_json::to_vec(&json!({
            "readers":readers, "reaped":true, "model_calls":mind.asked(), "final_effect":false
        }))?,
    )?;
    std::fs::write(root.join("tool-recovery-checked"), b"verified")?;
    Ok(())
}

pub fn verify(receipt: &Value) {
    assert_eq!(receipt["completions"], 2);
    assert_eq!(receipt["running_tool_cancelled"], true);
    assert_eq!(receipt["post_cancel_recovery"], true);
}

pub const SCREENS: &[(&str, &str)] = &[
    ("tool-permission-screen.txt", "Allow"),
    ("tool-cancelled-screen.txt", "Generation stopped"),
    ("tool-recovered-screen.txt", "editor-tool-recovered"),
];
