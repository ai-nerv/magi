//! What the model is told before the conversation starts: the configured instructions, the facts
//! about this session, and the project's own `AGENTS.md`. Assembled once when the daemon starts —
//! a file that changed mid-session would change what the model was told with no sign of it.

use std::path::Path;

/// The file a repository uses to say how it wants to be worked on. One name, not a search across
/// five; a project that wants something else can `include` it from this.
const PROJECT_FILE: &str = "AGENTS.md";

/// How much of a project file is taken. It rides on every request in the session.
const PROJECT_LIMIT: usize = 32_000;

/// Where a session sits among the agents of its run, which decides what it is told about working
/// with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seat {
    /// It may not start children, so there is nothing to say.
    Solo,
    /// It may start children and nobody started it: the one that decides whether to coordinate.
    Lead,
    /// Another agent started it with a brief.
    Worker,
}

impl Seat {
    /// From the two facts the harness has at start: whether `spawn` is allowed here, and whether a
    /// parent started this session.
    #[must_use]
    pub fn of(can_spawn: bool, has_parent: bool) -> Self {
        match (can_spawn, has_parent) {
            (_, true) => Self::Worker,
            (true, false) => Self::Lead,
            (false, false) => Self::Solo,
        }
    }

    fn guidance(self) -> Option<&'static str> {
        match self {
            Self::Solo => None,
            Self::Lead => Some(LEAD),
            Self::Worker => Some(WORKER),
        }
    }

    /// A prompt's aside with the size check added for a lead, so the choice to coordinate is made
    /// next to the task rather than only in the system prompt, where it weighs less.
    #[must_use]
    pub fn remind(self, aside: String) -> String {
        if self != Self::Lead {
            return aside;
        }
        if aside.is_empty() {
            REMINDER.to_owned()
        } else {
            format!("{aside}\n\n{REMINDER}")
        }
    }
}

/// The size check a lead's prompt carries.
const REMINDER: &str = "Before you start: is this big, with several components or more than about \
three new files? Then coordinate: write the plan and contract to PLAN.md, spawn one agent per part \
with a complete brief, wait, and verify the whole; do not write the parts yourself. If it is small, \
just do it.";

/// What a session that may start children is told: when to do the work, and how to coordinate it.
const LEAD: &str = r"# Working with other agents

You can start child agents with `spawn` and talk to them with the `agent` tool. Before you start a
task, decide whether it is yours to do or yours to coordinate, and decide by its size, not by
whether you could do it alone: you usually could, and that is not the question.

Do it yourself when it is small or tightly coupled: a quick fix, one file, or steps that each need
the previous one's context. Spawning costs time and tokens and buys nothing there.

Coordinate when it is big: it has several components, or needs more than about three new files. A
web app with a data layer, an API, a UI and tests is big; so is a change across several modules, or
a survey of a large codebase. Then you are the coordinator, not the implementer, and writing a
component yourself is a mistake:

1. Plan first. Settle the shared decisions yourself before anyone starts: the layout, the
   interfaces between the parts (function signatures, API routes and payloads, data shapes), and
   the commands that build and test it. Write them down in the project, for example in `PLAN.md`,
   so every agent works to the same contract, and scaffold whatever they all depend on.
2. Split along those seams into two to five parts, each with its own files. Two agents must never
   edit the same file.
3. Spawn one agent per part, all at once, each with a complete brief: the goal, the files it owns,
   the contract it must follow, its constraints, how to check its own work, and what to report
   back. It sees none of this conversation, so everything it needs goes in the brief.
4. While they work, do not write their parts yourself. Say what you are waiting for and end your
   turn; you are woken as each one finishes.
5. When one finishes you are told it handed in a report. Read it with the `agent` tool, verb
   `report`, who that agent; a long one comes a page at a time. Then integrate and verify the
   whole yourself: build it, run the tests, try it.
   Send each failure to its owner with `ask`, including the exact error, or spawn a fixer with it,
   and verify again.
6. Report what each agent did and whether the result works, from what they sent and what you
   checked, never from what you expected.";

/// What a session another agent started is told: do the brief, stay in its files, report back.
const WORKER: &str = r"# Working as a subagent

Another agent started you with a brief, and you are one part of a larger task. Do what the brief
asks: stay within the files it gives you, follow the contract it names, and check your own work the
way it says. Do not edit files that belong to other agents; if the brief is wrong, or something
outside your files blocks you, say so in your report instead. When you are done, hand in your
report with the `agent` tool, verb `report`: the whole of it in `message`, however long. A report
is not a message. Never send it with `send`, and never cut it into parts: a message is capped, and
the pieces arrive in your lead's conversation mixed up with everybody else's. Keep `send` for a
line saying you are blocked or need something. Start agents of your own only when your brief
is itself big and splits into independent parts; then plan first, give each its own files and a
complete brief, and verify their work before you report.";

/// Build the system prompt for a session rooted at `cwd`. `instructions` is what the configuration
/// said; `None` is a broken install rather than a choice, so the facts still go out. The guidance
/// for `seat` comes after the instructions, so a configuration that replaces them still gets it.
#[must_use]
pub fn assemble(instructions: Option<&str>, seat: Seat, cwd: &Path, now: &str) -> Option<String> {
    assemble_as(instructions, seat, None, cwd, now)
}

/// The same, for a session in a role the configuration describes: `role` is its name and what it is
/// told, set after the seat's guidance so a role narrows how the session works rather than replacing it.
#[must_use]
pub fn assemble_as(
    instructions: Option<&str>,
    seat: Seat,
    role: Option<(&str, &str)>,
    cwd: &Path,
    now: &str,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = instructions.map(str::trim).filter(|t| !t.is_empty()) {
        parts.push(text.to_owned());
    }
    if let Some(guidance) = seat.guidance() {
        parts.push(guidance.to_owned());
    }
    if let Some((name, told)) = role.filter(|(_, told)| !told.trim().is_empty()) {
        parts.push(format!("# Your role: {name}\n\n{}", told.trim()));
    }
    parts.push(environment(cwd, now));
    if let Some(project) = project_notes(cwd) {
        parts.push(project);
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The facts about this session, stated rather than left to be discovered.
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

/// The branch, when the directory is a repository and is on one. `branch --show-current` rather
/// than `rev-parse HEAD`, which fails with no commits and answers "HEAD" on a detached checkout.
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

/// What the project asked for, if it asked. Read from the session root only, not searched upward:
/// a file two directories above the one you work in is one you did not know you were agreeing to.
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
        let built = assemble(Some("Be terse."), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(built.starts_with("Be terse."), "{built}");
    }

    #[test]
    fn the_session_says_where_it_is() {
        let dir = scratch("facts");
        let built = assemble(Some("x"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains(&dir.display().to_string()), "{built}");
        assert!(built.contains("Platform: linux"), "{built}");
        assert!(built.contains("Today: 2026-08-27"), "{built}");
    }

    #[test]
    fn a_project_file_is_included_when_there_is_one() {
        let dir = scratch("agents");
        std::fs::write(dir.join(PROJECT_FILE), "Use tabs. We are monsters.").expect("write");
        let built = assemble(Some("x"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("Use tabs."), "{built}");
        assert!(
            built.contains(PROJECT_FILE),
            "it says where that came from: {built}"
        );
    }

    #[test]
    fn no_project_file_adds_no_section() {
        let dir = scratch("bare");
        let built = assemble(Some("x"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(!built.contains(PROJECT_FILE), "{built}");
    }

    #[test]
    fn an_empty_project_file_is_not_a_section_either() {
        let dir = scratch("empty-agents");
        std::fs::write(dir.join(PROJECT_FILE), "   \n\n").expect("write");
        let built = assemble(Some("x"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(!built.contains(PROJECT_FILE), "{built}");
    }

    #[test]
    fn a_very_long_project_file_is_cut_and_says_so() {
        let dir = scratch("long-agents");
        std::fs::write(dir.join(PROJECT_FILE), "x".repeat(PROJECT_LIMIT * 2)).expect("write");
        let built = assemble(Some("i"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("(truncated)"), "it admits the cut");
        assert!(built.len() < PROJECT_LIMIT * 2, "and actually cut it");
    }

    #[test]
    fn a_broken_install_still_states_the_facts() {
        let dir = scratch("noconfig");
        let built = assemble(None, Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("Working directory"), "{built}");
    }

    #[test]
    fn a_repository_says_which_branch() {
        let dir = scratch("branch");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git")
        };
        git(&["init", "--initial-branch=trunk"]);
        let built = assemble(Some("x"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("Git branch: trunk"), "{built}");
    }

    #[test]
    fn a_plain_directory_claims_no_branch() {
        let dir = scratch("nogit");
        let built = assemble(Some("x"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(!built.contains("Git branch"), "{built}");
    }

    #[test]
    fn a_lead_is_told_how_to_coordinate_after_its_own_instructions() {
        let dir = scratch("lead");
        let built = assemble(Some("Be terse."), Seat::Lead, &dir, "2026-08-27").expect("a prompt");
        assert!(
            built.starts_with("Be terse."),
            "the configuration comes first: {built}"
        );
        assert!(built.contains("# Working with other agents"), "{built}");
        assert!(
            built.contains("coordinator, not the implementer"),
            "{built}"
        );
        assert!(!built.contains("# Working as a subagent"), "{built}");
    }

    #[test]
    fn a_worker_is_told_to_do_its_brief_and_report_back() {
        let dir = scratch("worker");
        let built = assemble(Some("x"), Seat::Worker, &dir, "2026-08-27").expect("a prompt");
        assert!(built.contains("# Working as a subagent"), "{built}");
        assert!(!built.contains("# Working with other agents"), "{built}");
        // A result is handed in, not sent: a worker told to `send` its report cut it into
        // a dozen capped messages, which arrived in its lead's conversation out of order.
        assert!(built.contains("verb `report`"), "{built}");
        assert!(built.contains("never cut it into parts"), "{built}");
        assert!(!built.contains("send your report"), "{built}");
    }

    #[test]
    fn a_session_that_cannot_spawn_is_told_nothing_about_agents() {
        let dir = scratch("solo");
        let built = assemble(Some("x"), Seat::Solo, &dir, "2026-08-27").expect("a prompt");
        assert!(!built.contains("# Working"), "{built}");
    }

    #[test]
    fn the_seat_follows_from_spawning_and_parentage() {
        assert_eq!(Seat::of(true, false), Seat::Lead);
        assert_eq!(Seat::of(true, true), Seat::Worker);
        assert_eq!(Seat::of(false, true), Seat::Worker);
        assert_eq!(Seat::of(false, false), Seat::Solo);
    }

    #[test]
    fn only_a_lead_is_reminded_to_size_the_task() {
        assert!(Seat::Lead.remind(String::new()).contains("coordinate"));
        assert_eq!(Seat::Worker.remind("brief".to_owned()), "brief");
        assert_eq!(Seat::Solo.remind(String::new()), "");
        // After whatever else the prompt carries, not instead of it.
        let both = Seat::Lead.remind("named".to_owned());
        assert!(both.starts_with("named\n\n"), "{both}");
    }

    #[test]
    fn a_role_adds_its_instructions_after_the_seat() {
        let dir = scratch("role");
        let told = Some(("reviewer", "Edit nothing."));
        let built =
            assemble_as(Some("x"), Seat::Worker, told, &dir, "2026-08-27").expect("a prompt");
        let seat = built.find("# Working as a subagent").expect("the seat");
        let role = built.find("# Your role: reviewer").expect("the role");
        assert!(seat < role, "{built}");
        assert!(built.contains("Edit nothing."), "{built}");
    }
}
