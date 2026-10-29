use super::*;
use magi_testkit::mind::{call_lines, stop_line, text_line};
use serde_json::json;

pub fn provider(root: &Path, name: &str) -> Mind {
    let read = call_lines(
        "read",
        "read",
        &json!({"path":root.join("a/permit-note")}).to_string(),
    );
    let shell = call_lines(
        "shell",
        "shell",
        &json!({"command":format!("touch '{}'", root.join("a/forbidden").display())}).to_string(),
    );
    let allowed = [text_line("editor-allow-complete"), stop_line()];
    let denied = [text_line("editor-deny-read-complete"), stop_line()];
    let refused = [text_line("editor-deny-shell-complete"), stop_line()];
    let turns = [
        &read[..],
        &allowed[..],
        &read[..],
        &denied[..],
        &shell[..],
        &refused[..],
    ];
    let borrowed: Vec<Vec<&str>> = turns
        .iter()
        .map(|turn| turn.iter().map(String::as_str).collect())
        .collect();
    let slices: Vec<&[&str]> = borrowed.iter().map(Vec::as_slice).collect();
    Mind::turns(name, &slices)
}

pub async fn steps(root: &Path, mind: &Mind) -> Result<()> {
    marker(root, "allow-ready").await?;
    ensure!(mind.asked() == 1, "tool waits for editor approval");
    std::fs::write(root.join("allow-go"), b"allow once")?;
    marker(root, "deny-read-ready").await?;
    ensure!(mind.asked() == 3, "allow once requires a new read approval");
    ensure!(
        mind.asks()[1].contains("editor-allowed-read"),
        "allowed native result reaches the provider"
    );
    std::fs::write(root.join("a/permit-note"), "editor-never-read")?;
    std::fs::write(root.join("deny-read-go"), b"reject read")?;
    marker(root, "deny-shell-ready").await?;
    ensure!(mind.asked() == 5, "one followup after rejected read");
    ensure!(
        !mind.asks()[3].contains("editor-never-read"),
        "rejected read never reaches the provider"
    );
    ensure!(
        !root.join("a/forbidden").exists(),
        "shell has no effects before permission"
    );
    std::fs::write(root.join("deny-shell-go"), b"reject shell")?;
    marker(root, "permissions-complete").await?;
    ensure!(
        mind.asked() == 6,
        "exactly three native tool rounds and followups"
    );
    ensure!(
        !root.join("a/forbidden").exists(),
        "rejected shell has no effects"
    );
    ensure!(
        !mind.heard().contains("editor-never-read"),
        "no denied file content in any request"
    );
    std::fs::write(root.join("permissions-checked"), b"verified")?;
    Ok(())
}

pub fn verify(receipt: &Value) {
    assert_eq!(receipt["completions"], 3);
    assert_eq!(receipt["allowed_once"], 1);
    assert_eq!(receipt["rejected_once"], 2);
    let ids = receipt["permission_ids"]
        .as_array()
        .expect("permission identifiers");
    assert_eq!(ids.len(), 3);
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert!(
        receipt["buffer"]
            .as_str()
            .expect("chat")
            .contains("editor-deny-shell-complete")
    );
}

pub const SCREENS: &[(&str, &str)] = &[
    ("allow-pending-screen.txt", "Allow"),
    ("deny-read-screen.txt", "Refuse"),
    ("deny-shell-screen.txt", "Refuse"),
    (
        "permissions-completed-screen.txt",
        "editor-deny-shell-complete",
    ),
];
