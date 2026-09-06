//! What the model is told before the conversation starts.
//!
//! Three parts, from three places, because they answer to different people. The instructions
//! are configuration and belong to whoever installed magi. The facts — which directory, which
//! platform, what day — are neither configuration nor conversation: nobody has an opinion
//! about the working directory, and a model that has to ask is one that will guess. The
//! project's own `AGENTS.md` belongs to the repository and travels with it.
//!
//! Assembled once when the daemon starts rather than per turn. A file that changed mid-session
//! would silently change what the model was told between one message and the next, and the
//! transcript would give no sign of it.

use std::path::Path;

/// The file a repository uses to say how it wants to be worked on.
///
/// One name, not a search across five. Every agent that reads a different one has made the
/// convention worse, and a project that wants magi to read something else can `include` it
/// from this.
const PROJECT_FILE: &str = "AGENTS.md";

/// How much of a project file is taken.
///
/// Generous, and bounded: this rides on every request in the session, so a repository that
/// checked in a novel would pay for it on every turn.
const PROJECT_LIMIT: usize = 32_000;

/// Build the system prompt for a session rooted at `cwd`.
///
/// `instructions` is what the configuration said; `None` means a build with no system config
/// at all, which is a broken install rather than a choice, so the facts still go out.
#[must_use]
pub fn assemble(instructions: Option<&str>, cwd: &Path, now: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = instructions.map(str::trim).filter(|t| !t.is_empty()) {
        parts.push(text.to_owned());
    }
    parts.push(environment(cwd, now));
    if let Some(project) = project_notes(cwd) {
        parts.push(project);
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The facts about this session.
///
/// Stated rather than left to be discovered: a model that does not know its working directory
/// spends its first tool call running `pwd`, and one that does not know the platform writes
/// the wrong flags for `sed`.
fn environment(cwd: &Path, now: &str) -> String {
    let mut lines = vec![
        "# Environment".to_owned(),
        String::new(),
        format!("- Working directory: {}", cwd.display()),
        format!("- Platform: {}", std::env::consts::OS),
        format!("- Today: {now}"),
    ];
    if let Some(branch) = git_branch(cwd) {
        lines.push(format!("- Git branch: {branch}"));
    }
    lines.join("\n")
}

/// The branch, when the directory is a repository and is on one.
///
/// `branch --show-current` rather than `rev-parse HEAD`, which fails outright in a repository
/// with no commits yet and answers the literal string "HEAD" on a detached checkout.
fn git_branch(cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(cwd)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|branch| !branch.is_empty())
}

/// What the project asked for, if it asked.
///
/// Read from the session root only, not searched upward: a file two directories above the one
/// you are working in is a file you did not know you were agreeing to.
fn project_notes(cwd: &Path) -> Option<String> {
    let text = std::fs::read_to_string(cwd.join(PROJECT_FILE)).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let kept: String = text.chars().take(PROJECT_LIMIT).collect();
    let truncated = kept.len() < text.len();
    Some(format!(
        "# {PROJECT_FILE}\n\nThe project this session is rooted in asks for the following.\n\n{kept}{}",
        if truncated { "\n\n(truncated)" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-system", name)
    }

    #[test]
    fn the_instructions_come_first() {
        let dir = scratch("order");
        let built = assemble(Some("Be terse."), &dir, "2026-08-27").expect("a prompt");
        assert!(built.starts_with("Be terse."), "{built}");
    }

    #[test]
    fn the_session_says_where_it_is() {
        // A model that does not know its working directory spends its first tool call on `pwd`.
        let dir = scratch("facts");
        let built = assemble(Some("x"), &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains(&dir.display().to_string()), "{built}");
        assert!(built.contains("Platform: linux"), "{built}");
        assert!(built.contains("Today: 2026-08-27"), "{built}");
    }

    #[test]
    fn a_project_file_is_included_when_there_is_one() {
        let dir = scratch("agents");
        std::fs::write(dir.join(PROJECT_FILE), "Use tabs. We are monsters.").expect("write");
        let built = assemble(Some("x"), &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("Use tabs."), "{built}");
        assert!(
            built.contains(PROJECT_FILE),
            "it says where that came from: {built}"
        );
    }

    #[test]
    fn no_project_file_adds_no_section() {
        let dir = scratch("bare");
        let built = assemble(Some("x"), &dir, "2026-08-27").expect("a prompt");
        assert!(!built.contains(PROJECT_FILE), "{built}");
    }

    #[test]
    fn an_empty_project_file_is_not_a_section_either() {
        // A repository that touched the file and wrote nothing has said nothing.
        let dir = scratch("empty-agents");
        std::fs::write(dir.join(PROJECT_FILE), "   \n\n").expect("write");
        let built = assemble(Some("x"), &dir, "2026-08-27").expect("a prompt");
        assert!(!built.contains(PROJECT_FILE), "{built}");
    }

    #[test]
    fn a_very_long_project_file_is_cut_and_says_so() {
        // It rides on every request in the session, so a checked-in novel is paid for per turn.
        let dir = scratch("long-agents");
        std::fs::write(dir.join(PROJECT_FILE), "x".repeat(PROJECT_LIMIT * 2)).expect("write");
        let built = assemble(Some("i"), &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("(truncated)"), "it admits the cut");
        assert!(built.len() < PROJECT_LIMIT * 2, "and actually cut it");
    }

    #[test]
    fn a_broken_install_still_states_the_facts() {
        // No configuration at all is not a request for silence; it is an install to fix. The
        // model still needs to know where it is.
        let dir = scratch("noconfig");
        let built = assemble(None, &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("Working directory"), "{built}");
    }

    #[test]
    fn a_repository_says_which_branch() {
        // The one fact here that changes during a session, and the one a model most often
        // needs before it offers to commit anything.
        let dir = scratch("branch");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git")
        };
        git(&["init", "--initial-branch=trunk"]);
        let built = assemble(Some("x"), &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("Git branch: trunk"), "{built}");
    }

    #[test]
    fn a_plain_directory_claims_no_branch() {
        let dir = scratch("nogit");
        let built = assemble(Some("x"), &dir, "2026-08-27").expect("a prompt");
        assert!(!built.contains("Git branch"), "{built}");
    }
}
