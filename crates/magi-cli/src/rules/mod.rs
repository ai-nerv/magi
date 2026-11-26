//! Compile imperative rules in `AGENTS.md`, `CLAUDE.md`, and `.claude/rules/*.md` into enforced
//! `magi.deny` and `magi.ask` permission grants.

mod parse;

use magi_ipc::family::Reply;
use magi_proto::permit::{Grant, Scope};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compiled {
    pub file: String,
    pub line: usize,
    pub text: String,
    pub kind: RuleKind,
    pub grant: Grant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProseOnly {
    pub file: String,
    pub line: usize,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub root: String,
    pub sources: Vec<String>,
    pub compiled: Vec<Compiled>,
    pub prose_only: Vec<ProseOnly>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub root: String,
    pub deny: Vec<Grant>,
    pub ask: Vec<Grant>,
}

/// Compile rules for `root` from `AGENTS.md`, `CLAUDE.md`, and `.claude/rules/*.md`.
#[must_use]
pub fn compile_project(root: &Path) -> Report {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut report = Report {
        root: canonical.display().to_string(),
        ..Report::default()
    };
    let mut seen: BTreeSet<(u8, String, String)> = BTreeSet::new();

    for path in parse::discover_sources(&canonical) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&canonical)
            .unwrap_or(&path)
            .display()
            .to_string();
        report.sources.push(rel.clone());
        parse::parse_markdown(&canonical, &rel, &content, &mut seen, &mut report);
    }
    report
}

/// Load applied rules for `cwd` from `$XDG_CONFIG_HOME/magi/rules/`, if any exist.
#[must_use]
pub fn load_applied(cwd: &Path) -> Applied {
    let Some(dir) = crate::config::config_dir() else {
        return Applied::default();
    };
    load_applied_in(&dir, cwd)
}

fn load_applied_in(config_dir: &Path, cwd: &Path) -> Applied {
    let path = store_path_in(config_dir, cwd);
    let Ok(bytes) = std::fs::read(&path) else {
        return Applied::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Save `report`'s compiled grants for `cwd` in the user's configuration directory.
pub fn save_applied(cwd: &Path, report: &Report) -> std::io::Result<PathBuf> {
    let dir = crate::config::config_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no configuration directory available for rules",
        )
    })?;
    save_applied_in(&dir, cwd, report)
}

fn save_applied_in(config_dir: &Path, cwd: &Path, report: &Report) -> std::io::Result<PathBuf> {
    let path = store_path_in(config_dir, cwd);
    if let Some(parent) = path.parent() {
        magi_ipc::private_dirs(parent)?;
    }
    let applied = Applied {
        root: report.root.clone(),
        deny: report
            .compiled
            .iter()
            .filter(|c| c.kind == RuleKind::Deny)
            .map(|c| c.grant.clone())
            .collect(),
        ask: report
            .compiled
            .iter()
            .filter(|c| c.kind == RuleKind::Ask)
            .map(|c| c.grant.clone())
            .collect(),
    };
    let json = serde_json::to_vec_pretty(&applied).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Remove applied rules for `cwd`, returning true if a file was removed.
pub fn clear_applied(cwd: &Path) -> std::io::Result<bool> {
    let Some(dir) = crate::config::config_dir() else {
        return Ok(false);
    };
    clear_applied_in(&dir, cwd)
}

fn clear_applied_in(config_dir: &Path, cwd: &Path) -> std::io::Result<bool> {
    let path = store_path_in(config_dir, cwd);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Execute `magi rules [--apply | --clear | --lua]`.
pub fn run(apply: bool, clear: bool, lua: bool, how: crate::verbs::As) {
    let Ok(cwd) = std::env::current_dir() else {
        fail("could not determine working directory", how);
        return;
    };
    if clear {
        match clear_applied(&cwd) {
            Ok(removed) => {
                if how.framed() {
                    crate::verbs::say(&Reply::of(serde_json::json!({ "cleared": removed })), how);
                } else if removed {
                    println!("cleared applied rules for {}", cwd.display());
                } else {
                    println!("no applied rules for {}", cwd.display());
                }
            }
            Err(why) => fail(&why.to_string(), how),
        }
        return;
    }

    let report = compile_project(&cwd);
    if lua {
        let snippet = render_lua(&report);
        if how.framed() {
            crate::verbs::say(&Reply::of(serde_json::json!({ "lua": snippet })), how);
        } else {
            print!("{snippet}");
        }
        return;
    }

    let applied_path = if apply {
        match save_applied(&cwd, &report) {
            Ok(path) => Some(path),
            Err(why) => {
                fail(&why.to_string(), how);
                return;
            }
        }
    } else {
        None
    };

    if how.framed() {
        let value = serde_json::json!({
            "root": report.root,
            "sources": report.sources,
            "compiled": report.compiled,
            "prose_only": report.prose_only,
            "applied": applied_path.as_ref().map(|p| p.display().to_string()),
        });
        crate::verbs::say(&Reply::of(value), how);
        return;
    }

    if report.sources.is_empty() {
        println!(
            "no AGENTS.md, CLAUDE.md, or .claude/rules/*.md found in {}",
            report.root
        );
        return;
    }
    println!(
        "sources: {} ({} enforceable, {} prose-only)",
        report.sources.join(", "),
        report.compiled.len(),
        report.prose_only.len()
    );
    for item in &report.compiled {
        let kind = match item.kind {
            RuleKind::Deny => "deny",
            RuleKind::Ask => "ask ",
        };
        println!(
            "  [{kind}] {} ({}:{}) <- {}",
            describe_grant(&item.grant),
            item.file,
            item.line,
            item.text
        );
    }
    for item in &report.prose_only {
        println!(
            "  [prose] {}:{} ({}) <- {}",
            item.file, item.line, item.reason, item.text
        );
    }
    if let Some(path) = applied_path {
        println!(
            "applied {} rule(s) to {}",
            report.compiled.len(),
            path.display()
        );
    } else if !report.compiled.is_empty() {
        println!("run `magi rules --apply` to enforce these rules, or `magi rules --lua` for Lua.");
    }
}

fn fail(why: &str, how: crate::verbs::As) {
    if how.framed() {
        crate::verbs::say(&Reply::refused(why), how);
    } else {
        eprintln!("magi rules: {why}");
    }
}

fn store_path_in(config_dir: &Path, cwd: &Path) -> PathBuf {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let hash = fnv1a_hex(canonical.to_string_lossy().as_bytes());
    config_dir.join("rules").join(format!("{hash}.json"))
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub(super) fn describe_grant(grant: &Grant) -> String {
    match &grant.scope {
        Scope::Directory { path } => format!("{} {path}", grant.verb),
        Scope::Program { program } => format!("{} `{program}`", grant.verb),
        Scope::Anything => format!("{} anything", grant.verb),
        Scope::Once | Scope::Exact => grant.verb.clone(),
    }
}

#[must_use]
pub fn render_lua(report: &Report) -> String {
    let mut out = String::new();
    let emit = |out: &mut String, table: &str, kind: RuleKind| {
        let items: Vec<&Grant> = report
            .compiled
            .iter()
            .filter(|c| c.kind == kind)
            .map(|c| &c.grant)
            .collect();
        if items.is_empty() {
            return;
        }
        out.push_str(&format!("magi.{table} = {{\n"));
        for grant in items {
            match &grant.scope {
                Scope::Directory { path } => {
                    out.push_str(&format!(
                        "  {{ verb = {:?}, directory = {:?} }},\n",
                        grant.verb, path
                    ));
                }
                Scope::Program { program } => {
                    out.push_str(&format!(
                        "  {{ verb = {:?}, program = {:?} }},\n",
                        grant.verb, program
                    ));
                }
                Scope::Anything => {
                    out.push_str(&format!(
                        "  {{ verb = {:?}, anything = true }},\n",
                        grant.verb
                    ));
                }
                Scope::Once | Scope::Exact => {}
            }
        }
        out.push_str("}\n");
    };
    emit(&mut out, "deny", RuleKind::Deny);
    emit(&mut out, "ask", RuleKind::Ask);
    if out.is_empty() {
        out.push_str("-- no enforceable rules compiled\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn compiles_path_and_program_rules_and_keeps_subcommands_as_prose() {
        let dir = Scratch::new("magi-rules", "compile");
        let agents = dir.join("AGENTS.md");
        std::fs::write(
            &agents,
            "\
# Project rules
- Never edit the migrations folder.
- Do not modify `Cargo.lock` or `.env`.
- Never run `sudo`.
- Never run `git push --force`.
- Ask before editing `config/prod.toml`.
- Always write idiomatic Rust.
```
Never edit src/
```
",
        )
        .expect("write");

        let report = compile_project(&dir);
        let root = dir.canonicalize().expect("canon");
        assert_eq!(report.sources, vec!["AGENTS.md"]);

        let has = |kind: RuleKind, verb: &str, scope: Scope| {
            report
                .compiled
                .iter()
                .any(|c| c.kind == kind && c.grant.verb == verb && c.grant.scope == scope)
        };
        assert!(has(
            RuleKind::Deny,
            "write",
            Scope::Directory {
                path: root.join("migrations").display().to_string()
            }
        ));
        assert!(has(
            RuleKind::Deny,
            "write",
            Scope::Directory {
                path: root.join("Cargo.lock").display().to_string()
            }
        ));
        assert!(has(
            RuleKind::Deny,
            "write",
            Scope::Directory {
                path: root.join(".env").display().to_string()
            }
        ));
        assert!(has(
            RuleKind::Deny,
            "run",
            Scope::Program {
                program: "sudo".to_owned()
            }
        ));
        assert!(has(
            RuleKind::Ask,
            "write",
            Scope::Directory {
                path: root.join("config/prod.toml").display().to_string()
            }
        ));
        assert!(
            !has(
                RuleKind::Deny,
                "run",
                Scope::Program {
                    program: "git".to_owned()
                }
            ),
            "subcommand rule must not widen to blocking all git"
        );
        assert!(
            !has(
                RuleKind::Deny,
                "write",
                Scope::Directory {
                    path: root.join("src").display().to_string()
                }
            ),
            "fenced code blocks must be ignored"
        );
        assert!(
            report
                .prose_only
                .iter()
                .any(|p| p.text.contains("git push --force")),
            "subcommand rule reported as prose-only"
        );
    }

    #[test]
    fn renders_valid_lua_tables() {
        let dir = Scratch::new("magi-rules", "lua");
        std::fs::write(dir.join("CLAUDE.md"), "Never run `sudo`.\n").expect("write");
        let report = compile_project(&dir);
        let lua = render_lua(&report);
        assert!(lua.contains("magi.deny = {"), "{lua}");
        assert!(lua.contains("program = \"sudo\""), "{lua}");
    }

    #[test]
    fn saves_loads_and_clears_applied_rules_in_private_config_dir() {
        let project = Scratch::new("magi-rules", "project");
        let config = Scratch::new("magi-rules", "config");
        std::fs::write(
            project.join("AGENTS.md"),
            "- Never edit `migrations/`.\n- Ask before modifying `Cargo.lock`.\n",
        )
        .expect("write");

        let report = compile_project(&project);
        let saved = save_applied_in(&config, &project, &report).expect("save");
        assert!(saved.is_file());

        let loaded = load_applied_in(&config, &project);
        assert_eq!(loaded.deny.len(), 1);
        assert_eq!(loaded.ask.len(), 1);
        assert!(
            loaded.deny[0].names(&magi_proto::permit::Action::Write {
                path: project
                    .canonicalize()
                    .expect("canon")
                    .join("migrations/001.sql")
                    .display()
                    .to_string(),
            })
        );

        assert!(clear_applied_in(&config, &project).expect("clear"));
        assert!(!clear_applied_in(&config, &project).expect("clear again"));
        assert!(load_applied_in(&config, &project).deny.is_empty());
    }
}
