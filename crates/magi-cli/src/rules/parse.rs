//! Deterministic markdown rule discovery and classification for `magi rules`.

use super::{Compiled, ProseOnly, Report, RuleKind, describe_grant};
use magi_proto::permit::{Grant, Scope};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub(super) fn discover_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = root.join(name);
        if path.is_file() {
            files.push(path);
        }
    }
    let rules_dir = root.join(".claude").join("rules");
    if let Ok(entries) = std::fs::read_dir(&rules_dir) {
        let mut extras: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
            .collect();
        extras.sort();
        files.extend(extras);
    }
    files
}

pub(super) fn parse_markdown(
    root: &Path,
    rel: &str,
    content: &str,
    seen: &mut BTreeSet<(u8, String, String)>,
    report: &mut Report,
) {
    let mut in_fence = false;
    for (idx, raw) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cleaned = strip_markdown_prefix(trimmed);
        for sentence in split_sentences(cleaned) {
            classify_sentence(root, rel, line_no, sentence, seen, report);
        }
    }
}

fn strip_markdown_prefix(line: &str) -> &str {
    let mut s = line.trim();
    loop {
        if let Some(rest) = s
            .strip_prefix("- ")
            .or_else(|| s.strip_prefix("* "))
            .or_else(|| s.strip_prefix("+ "))
            .or_else(|| s.strip_prefix("> "))
        {
            s = rest.trim_start();
            continue;
        }
        if let Some(dot) = s.find(". ")
            && dot > 0
            && dot <= 3
            && s[..dot].chars().all(|c| c.is_ascii_digit())
        {
            s = s[dot + 2..].trim_start();
            continue;
        }
        break;
    }
    s
}

fn split_sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut start = 0;
    let mut in_tick = false;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'`' {
            in_tick = !in_tick;
            continue;
        }
        if in_tick {
            continue;
        }
        let end = match b {
            b'!' | b';' => true,
            b'.' => bytes.get(i + 1).is_none_or(|n| n.is_ascii_whitespace()),
            _ => false,
        };
        if end {
            let part = line[start..=i].trim();
            if !part.is_empty() {
                out.push(part);
            }
            start = i + 1;
        }
    }
    let tail = line[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn classify_sentence(
    root: &Path,
    rel: &str,
    line_no: usize,
    sentence: &str,
    seen: &mut BTreeSet<(u8, String, String)>,
    report: &mut Report,
) {
    let lower = sentence.to_lowercase();
    let kind = if has_any(
        &lower,
        &[
            "ask before",
            "confirm before",
            "ask permission before",
            "check with me before",
            "always ask before",
            "require confirmation before",
        ],
    ) {
        RuleKind::Ask
    } else if has_any(
        &lower,
        &[
            "never ",
            "do not ",
            "don't ",
            "must not ",
            "forbidden to ",
            "disallow ",
        ],
    ) {
        RuleKind::Deny
    } else {
        return;
    };

    let write_verb = has_word(
        &lower,
        &[
            "edit",
            "modify",
            "touch",
            "change",
            "write",
            "overwrite",
            "delete",
            "remove",
            "alter",
            "update",
            "editing",
            "modifying",
            "touching",
            "changing",
            "writing",
            "deleting",
        ],
    );
    let read_verb = has_word(
        &lower,
        &["read", "open", "inspect", "cat", "reading", "opening"],
    );
    let run_verb = has_word(
        &lower,
        &[
            "run",
            "execute",
            "invoke",
            "call",
            "use",
            "running",
            "executing",
        ],
    );

    let mut grants = Vec::new();
    let mut subcommand_note: Option<String> = None;

    if write_verb || read_verb {
        let verb = if write_verb { "write" } else { "read" };
        for target in extract_paths(sentence, root) {
            grants.push(Grant {
                verb: verb.to_owned(),
                scope: Scope::Directory { path: target },
            });
        }
    }

    if grants.is_empty() && run_verb {
        for tick in backticks(sentence) {
            let trimmed = tick.trim();
            if trimmed.is_empty() || trimmed.contains('/') {
                continue;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            match parts.as_slice() {
                [program] if is_program_name(program) => {
                    grants.push(Grant {
                        verb: "run".to_owned(),
                        scope: Scope::Program {
                            program: (*program).to_owned(),
                        },
                    });
                }
                [program, ..] if parts.len() > 1 && is_program_name(program) => {
                    subcommand_note = Some(format!(
                        "subcommand rule `{trimmed}` cannot be narrowed below program `{program}`"
                    ));
                }
                _ => {}
            }
        }
    }

    if grants.is_empty() {
        let reason = subcommand_note
            .unwrap_or_else(|| "no concrete path or standalone program target".to_owned());
        report.prose_only.push(ProseOnly {
            file: rel.to_owned(),
            line: line_no,
            text: sentence.to_owned(),
            reason,
        });
        return;
    }

    for grant in grants {
        let key = (
            u8::from(kind == RuleKind::Ask),
            grant.verb.clone(),
            describe_grant(&grant),
        );
        if seen.insert(key) {
            report.compiled.push(Compiled {
                file: rel.to_owned(),
                line: line_no,
                text: sentence.to_owned(),
                kind,
                grant,
            });
        }
    }
}

fn has_any(text: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| text.contains(p))
}

fn has_word(text: &str, words: &[&str]) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
        .any(|tok| words.contains(&tok))
}

fn backticks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else {
            break;
        };
        out.push(&rest[..close]);
        rest = &rest[close + 1..];
    }
    out
}

fn extract_paths(sentence: &str, root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for tick in backticks(sentence) {
        if let Some(resolved) = resolve_path_token(tick.trim(), root, true)
            && !out.contains(&resolved)
        {
            out.push(resolved);
        }
    }
    if !out.is_empty() {
        return out;
    }
    for raw in sentence.split_whitespace() {
        let cleaned = raw.trim_matches(|c: char| ".,;:!?\"'()[]{}".contains(c));
        if let Some(resolved) = resolve_path_token(cleaned, root, false)
            && !out.contains(&resolved)
        {
            out.push(resolved);
        }
    }
    if !out.is_empty() {
        return out;
    }
    let words: Vec<&str> = sentence
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| ".,;:!?\"'`()[]{}".contains(c)))
        .collect();
    for pair in words.windows(2) {
        let [name, next] = pair else {
            continue;
        };
        let next = next.to_lowercase();
        if matches!(next.as_str(), "folder" | "directory" | "dir")
            && is_plain_dir_name(name)
            && let Some(resolved) = resolve_under_root(name, root)
            && !out.contains(&resolved)
        {
            out.push(resolved);
        }
    }
    out
}

fn resolve_path_token(token: &str, root: &Path, quoted: bool) -> Option<String> {
    if token.is_empty() || token.contains(char::is_whitespace) || token.contains("://") {
        return None;
    }
    let looks_like_path = token.contains('/')
        || token.starts_with('.')
        || has_known_extension(token)
        || (quoted && root.join(token).exists());
    if !looks_like_path {
        return None;
    }
    let stripped = token.trim_end_matches('/');
    if stripped.is_empty() {
        return None;
    }
    resolve_under_root(stripped, root)
}

fn resolve_under_root(raw: &str, root: &Path) -> Option<String> {
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let normalized = lexical(&joined);
    if !candidate.is_absolute() && !normalized.starts_with(root) {
        return None;
    }
    Some(normalized.display().to_string())
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn has_known_extension(token: &str) -> bool {
    let Some((_, ext)) = token.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_lowercase().as_str(),
        "lock" | "env" | "sql" | "toml" | "yaml" | "yml" | "json" | "key" | "pem" | "md" | "rs"
    )
}

fn is_plain_dir_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(
            name.to_lowercase().as_str(),
            "the" | "this" | "that" | "any" | "a" | "an" | "root" | "project" | "repo"
        )
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_program_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}
