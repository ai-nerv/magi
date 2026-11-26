//! Shell command analysis for permissions, powered by `rune`.
//!
//! Parses POSIX/bash commands into a lossless syntax tree in-process so every invoked program
//! (across pipelines, lists, conditionals, subshells, substitutions, and `sh -c` scripts) is
//! checked while quoted data strings never false-trigger deny rules.

use magi_proto::permit::{Grant, Scope};
use rune::ast::SimpleCommand;
use rune::{Node, Source};

const MAX_DEPTH: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub program: String,
    pub has_assignments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub commands: Vec<ParsedCommand>,
    pub ambiguous: bool,
}

/// Parse a shell command with `rune` and collect every invoked program.
#[must_use]
pub fn analyze(command: &str) -> Analysis {
    analyze_depth(command, 0)
}

/// Extract the first invoked program name for display / permission prompt subject.
#[must_use]
pub fn first_program(command: &str) -> String {
    analyze(command)
        .commands
        .into_iter()
        .next()
        .map(|cmd| cmd.program)
        .unwrap_or_default()
}

/// Whether `grants` cover every invoked command in `command` (fail-closed on ambiguity or env overrides).
#[must_use]
pub fn covers_run(grants: &[Grant], command: &str) -> bool {
    if grants
        .iter()
        .any(|g| g.verb == "run" && matches!(g.scope, Scope::Anything))
    {
        return true;
    }
    let analysis = analyze(command);
    if analysis.ambiguous || analysis.commands.is_empty() {
        return false;
    }
    if analysis.commands.iter().any(|cmd| cmd.has_assignments) {
        return false;
    }
    analysis.commands.iter().all(|cmd| {
        let base = basename(&cmd.program);
        grants.iter().any(|g| {
            g.verb == "run"
                && matches!(
                    &g.scope,
                    Scope::Program { program } if program == &cmd.program || program == base
                )
        })
    })
}

/// Whether a deny/ask `rule` names any real command invoked by `command`.
#[must_use]
pub fn names_run(rule: &Grant, command: &str) -> bool {
    if rule.verb != "run" {
        return false;
    }
    match &rule.scope {
        Scope::Anything => true,
        Scope::Program { program } => {
            let analysis = analyze(command);
            analysis.commands.iter().any(|cmd| {
                let base = basename(&cmd.program);
                base == program || cmd.program == *program
            })
        }
        _ => false,
    }
}

/// Whether `command` contains dynamic/unparseable invocations that require asking the user.
#[must_use]
pub fn is_ambiguous(command: &str) -> bool {
    analyze(command).ambiguous
}

fn analyze_depth(command: &str, depth: usize) -> Analysis {
    if depth >= MAX_DEPTH {
        return Analysis {
            commands: Vec::new(),
            ambiguous: true,
        };
    }
    let parsed = rune::parse(command);
    let source = Source::new(command);
    let mut commands = Vec::new();
    let mut ambiguous = !parsed.is_clean();
    visit_node(
        parsed.tree().root(),
        &source,
        depth,
        &mut commands,
        &mut ambiguous,
    );
    Analysis {
        commands,
        ambiguous,
    }
}

fn visit_node(
    node: &Node,
    source: &Source,
    depth: usize,
    commands: &mut Vec<ParsedCommand>,
    ambiguous: &mut bool,
) {
    if let Some(cmd) = SimpleCommand::cast(node) {
        extract_simple(cmd, source, depth, commands, ambiguous);
    }
    for child in node.nodes() {
        visit_node(child, source, depth, commands, ambiguous);
    }
}

fn extract_simple(
    cmd: SimpleCommand<'_>,
    source: &Source,
    depth: usize,
    commands: &mut Vec<ParsedCommand>,
    ambiguous: &mut bool,
) {
    let Some(name_word) = cmd.name() else {
        return;
    };
    let Some(program) = name_word.literal(source) else {
        *ambiguous = true;
        return;
    };
    if program.is_empty() || has_glob(&program) {
        *ambiguous = true;
        return;
    }
    let has_assignments = cmd.assignments().next().is_some();
    let base = basename(&program);
    if matches!(base, "eval" | "exec" | "source" | ".") {
        *ambiguous = true;
    }
    commands.push(ParsedCommand {
        program: program.clone(),
        has_assignments,
    });

    if matches!(base, "sh" | "bash" | "dash" | "zsh") {
        let mut args = cmd.arguments();
        while let Some(arg) = args.next() {
            let Some(flag) = arg.literal(source) else {
                continue;
            };
            if is_dash_c_flag(&flag) {
                if let Some(script_word) = args.next() {
                    if let Some(inner) = script_word.literal(source) {
                        let sub = analyze_depth(&inner, depth + 1);
                        commands.extend(sub.commands);
                        *ambiguous |= sub.ambiguous;
                    } else {
                        *ambiguous = true;
                    }
                }
                break;
            }
        }
    }
}

fn is_dash_c_flag(flag: &str) -> bool {
    let Some(rest) = flag.strip_prefix('-') else {
        return false;
    };
    !rest.is_empty()
        && !rest.starts_with('-')
        && rest.bytes().all(|b| b.is_ascii_alphabetic())
        && rest.contains('c')
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

fn has_glob(token: &str) -> bool {
    token.contains(['*', '?', '['])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(name: &str) -> Grant {
        Grant {
            verb: "run".to_owned(),
            scope: Scope::Program {
                program: name.to_owned(),
            },
        }
    }

    #[test]
    fn parses_compound_commands_and_ignores_quoted_arguments() {
        let compound = analyze("git diff && rm -rf /tmp/x");
        let names: Vec<&str> = compound
            .commands
            .iter()
            .map(|c| c.program.as_str())
            .collect();
        assert_eq!(names, vec!["git", "rm"]);
        assert!(!compound.ambiguous);

        let quoted = analyze(r#"git commit -m "remove rm && sudo from scripts""#);
        let qnames: Vec<&str> = quoted.commands.iter().map(|c| c.program.as_str()).collect();
        assert_eq!(qnames, vec!["git"]);
        assert!(!names_run(
            &prog("rm"),
            r#"git commit -m "remove rm && sudo""#
        ));
        assert!(!names_run(
            &prog("sudo"),
            r#"git commit -m "remove rm && sudo""#
        ));
        assert!(names_run(&prog("rm"), "git diff && rm -rf /"));
    }

    #[test]
    fn covers_pipeline_only_when_every_command_is_granted() {
        let git_only = vec![prog("git")];
        let git_and_head = vec![prog("git"), prog("head")];
        assert!(!covers_run(&git_only, "git log | head -n 10"));
        assert!(covers_run(&git_and_head, "git log | head -n 10"));
        assert!(!covers_run(&git_and_head, "git diff && rm -rf /"));
        assert!(!covers_run(&git_only, "PATH=/evil git status"));
        assert!(!covers_run(&git_only, "eval 'git status'"));
    }

    #[test]
    fn catches_subshells_substitutions_conditionals_and_sh_c() {
        assert!(names_run(&prog("rm"), "echo $(rm -rf /tmp/a)"));
        assert!(names_run(&prog("rm"), "echo `rm -rf /tmp/a`"));
        assert!(names_run(&prog("rm"), "diff <(cat a) >(rm -rf /tmp/a)"));
        assert!(names_run(&prog("rm"), "if true; then rm -rf /tmp/a; fi"));
        assert!(names_run(&prog("rm"), "sh -c 'true && /bin/rm -f x'"));
        assert!(names_run(&prog("rm"), r"\rm -rf ~"));
        assert!(names_run(&prog("rm"), r#"echo "${x:-$(rm -rf /tmp/a)}""#));
        assert!(is_ambiguous("$CMD --flag"));
        assert!(is_ambiguous("eval 'echo hi'"));
        assert!(is_ambiguous("echo 'unclosed"));
        assert!(!is_ambiguous("git log --format='%s [ok]'"));
    }
}
