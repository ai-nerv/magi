//! Higher-level session profiles: named operating modes (`planner`, `executor`, `architect`, ...)
//! bundling system prompt, model/thinking/helper overrides, skills, permission overlays, tool
//! filters, and which lower-level subagent roles (`magi.agents`) the session may spawn.

use magi_proto::judging::Mode;
use magi_proto::permit::{Grant, Scope};
use magi_tools::ToolFilter;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const FILE_MOST: u64 = 64 * 1024;

/// One higher-level session profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    /// Per-helper model overrides (`safety`, `memory`, `decision`, `summary`, ...).
    pub helpers: BTreeMap<String, String>,
    /// Skill names automatically loaded into this profile's system prompt.
    pub skills_preload: Vec<String>,
    /// Optional glob/name filter restricting which skills are available in this profile.
    pub skills_allow: Option<Vec<String>>,
    /// Permission mode override (`ask`, `edits`, `auto`, `locked`).
    pub mode: Option<Mode>,
    /// Ephemeral grants active only while this profile is selected.
    pub allow: Vec<Grant>,
    /// Extra always-ask rules layered on top of global `magi.ask`.
    pub ask: Vec<Grant>,
    /// Extra deny rules layered on top of global `magi.deny`.
    pub deny: Vec<Grant>,
    /// Tool allowlist or denylist enforced in `Registry`.
    pub tools: ToolFilter,
    /// Subset of `magi.agents` this profile may spawn (`None` = all, `Some([])` = none).
    pub agents: Option<Vec<String>>,
}

impl Profile {
    /// One-line summary shown in the `:profile` picker.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(model) = &self.model {
            parts.push(model.clone());
        }
        if let Some(mode) = self.mode {
            parts.push(mode.name().to_owned());
        }
        if let Some(desc) = &self.description {
            parts.push(desc.clone());
        }
        parts.join(" · ")
    }

    /// Effective tool filter: combines explicit tool allow/deny with hiding `spawn` when `agents`
    /// is explicitly empty (`agents = {}`).
    #[must_use]
    pub fn effective_tool_filter(&self) -> ToolFilter {
        if self.agents.as_deref() == Some(&[]) {
            self.tools.without("spawn")
        } else {
            self.tools.clone()
        }
    }

    /// Extra judging rules (`ask` + `deny`) contributed by this profile.
    #[must_use]
    pub fn judging_rules(&self) -> magi_proto::judging::Rules {
        magi_proto::judging::Rules {
            ask: self.ask.clone(),
            deny: self.deny.clone(),
        }
    }
}

/// Whether `name` matches a skill pattern supporting `*` wildcards.
#[must_use]
pub fn matches_skill_pattern(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    for (idx, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if idx == 0 {
            let Some(after) = rest.strip_prefix(part) else {
                return false;
            };
            rest = after;
        } else if idx + 1 == parts.len() {
            return rest.ends_with(part);
        } else {
            let Some(pos) = rest.find(part) else {
                return false;
            };
            rest = rest.get(pos + part.len()..).unwrap_or("");
        }
    }
    true
}

/// Whether `name` is permitted by an optional profile skill allowlist.
#[must_use]
pub fn skill_allowed(allow: Option<&[String]>, name: &str) -> bool {
    match allow {
        None => true,
        Some(patterns) => patterns
            .iter()
            .any(|pattern| matches_skill_pattern(pattern, name)),
    }
}

/// Parse a compact string grant shorthand such as `"write"`, `"read"`, `"run:git"`,
/// `"write:/tmp"`, or `"reach:example.com"`.
#[must_use]
pub fn parse_grant_shorthand(spec: &str) -> Option<Grant> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (verb, rest) = match trimmed.split_once(':') {
        Some((v, r)) => (v.trim(), Some(r.trim())),
        None => (trimmed, None),
    };
    if !matches!(verb, "read" | "write" | "run" | "reach") {
        return None;
    }
    let scope = match rest {
        None | Some("" | "*") => Scope::Anything,
        Some(target) if verb == "run" => Scope::Program {
            program: target.to_owned(),
        },
        Some(target) => Scope::Directory {
            path: target.to_owned(),
        },
    };
    Some(Grant {
        verb: verb.to_owned(),
        scope,
    })
}

/// Assemble the effective system prompt from the base prompt, optional profile prompt, and any
/// preloaded skills.
#[must_use]
pub fn compose_system(
    base: Option<&str>,
    profile: Option<&Profile>,
    preloaded_skills: &[(String, String)],
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(base) = base.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(base.to_owned());
    }
    if let Some(profile) = profile
        && let Some(prompt) = profile
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        parts.push(format!("# Profile: {}\n\n{prompt}", profile.name));
    }
    for (name, body) in preloaded_skills {
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            parts.push(format!("# Preloaded skill: {name}\n\n{trimmed}"));
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Discover drop-in Markdown profiles in `dir` (`<name>.md`), skipping symlinks and oversized files.
#[must_use]
pub fn load_dir(dir: &Path) -> Vec<Profile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_file() || meta.len() > FILE_MOST {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match parse_markdown(name, &text) {
            Ok(profile) => profiles.push(profile),
            Err(why) => eprintln!("magi: skipped profile {}: {why}", path.display()),
        }
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    profiles
}

/// Parse a drop-in Markdown profile with bounded YAML-like `---` frontmatter and Markdown prompt body.
pub fn parse_markdown(name: &str, text: &str) -> Result<Profile, String> {
    let mut profile = Profile {
        name: name.to_owned(),
        ..Profile::default()
    };
    let (front, body) = split_frontmatter(text)?;
    let mut tools_allow: Option<BTreeSet<String>> = None;
    let mut tools_deny: Option<BTreeSet<String>> = None;
    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw_val)) = trimmed.split_once(':') else {
            return Err(format!("invalid frontmatter line: {trimmed:?}"));
        };
        let key = key.trim();
        let val = raw_val.trim();
        match key {
            "description" => profile.description = non_empty(unquote(val)),
            "model" => profile.model = non_empty(unquote(val)),
            "thinking" => profile.thinking = non_empty(unquote(val)),
            "mode" => {
                let mode = unquote(val);
                profile.mode = Some(
                    Mode::named(&mode)
                        .ok_or_else(|| format!("unknown permission mode {mode:?}"))?,
                );
            }
            "safety" | "memory" | "decision" | "summary" => {
                if let Some(m) = non_empty(unquote(val)) {
                    profile.helpers.insert(key.to_owned(), m);
                }
            }
            "skills_preload" => profile.skills_preload = parse_list(val),
            "skills_allow" => profile.skills_allow = Some(parse_list(val)),
            "tools_allow" => tools_allow = Some(parse_list(val).into_iter().collect()),
            "tools_deny" => tools_deny = Some(parse_list(val).into_iter().collect()),
            "allow" => {
                profile.allow = parse_list(val)
                    .iter()
                    .filter_map(|s| parse_grant_shorthand(s))
                    .collect();
            }
            "ask" => {
                profile.ask = parse_list(val)
                    .iter()
                    .filter_map(|s| parse_grant_shorthand(s))
                    .collect();
            }
            "deny" => {
                profile.deny = parse_list(val)
                    .iter()
                    .filter_map(|s| parse_grant_shorthand(s))
                    .collect();
            }
            "agents" => profile.agents = Some(parse_list(val)),
            _ => {}
        }
    }
    profile.tools = match (tools_allow, tools_deny) {
        (Some(allow), _) => ToolFilter::Allow(allow),
        (None, Some(deny)) => ToolFilter::Deny(deny),
        (None, None) => ToolFilter::All,
    };
    profile.prompt = non_empty(body.trim().to_owned());
    Ok(profile)
}

fn split_frontmatter(text: &str) -> Result<(&str, &str), String> {
    let stripped = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"));
    let Some(rest) = stripped else {
        return Ok(("", text));
    };
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim() == "---" {
            let front = rest.get(..offset).unwrap_or("");
            let body = rest.get(offset + line.len()..).unwrap_or("");
            return Ok((front, body));
        }
        offset += line.len();
    }
    Err("unclosed YAML frontmatter (`---`)".to_owned())
}

fn unquote(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        return trimmed.get(1..trimmed.len() - 1).unwrap_or("").to_owned();
    }
    trimmed.to_owned()
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_list(raw: &str) -> Vec<String> {
    let inner = raw
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(raw.trim());
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(unquote)
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markdown_profile_with_all_fields() {
        let md = r#"---
description: Explores and designs a plan; edits nothing.
model: anthropic/claude-opus-4-6
thinking: high
mode: ask
safety: anthropic/claude-haiku-4-5
tools_deny: [write, edit]
allow: [read, "run:git"]
ask: ["run:git push"]
deny: [write, "run:rm"]
skills_preload: [plan-review]
skills_allow: ["plan-*", design]
agents: [explorer]
---
You are in planning mode. Do not modify files.
"#;
        let profile = parse_markdown("planner", md).expect("parse markdown");
        assert_eq!(profile.name, "planner");
        assert_eq!(profile.model.as_deref(), Some("anthropic/claude-opus-4-6"));
        assert_eq!(profile.thinking.as_deref(), Some("high"));
        assert_eq!(profile.mode, Some(Mode::Ask));
        assert_eq!(
            profile.helpers.get("safety").map(String::as_str),
            Some("anthropic/claude-haiku-4-5")
        );
        assert_eq!(
            profile.tools,
            ToolFilter::Deny(BTreeSet::from(["write".to_owned(), "edit".to_owned()]))
        );
        assert_eq!(profile.allow.len(), 2);
        assert_eq!(profile.deny.len(), 2);
        assert_eq!(profile.skills_preload, vec!["plan-review"]);
        assert_eq!(
            profile.skills_allow,
            Some(vec!["plan-*".to_owned(), "design".to_owned()])
        );
        assert_eq!(profile.agents, Some(vec!["explorer".to_owned()]));
        assert_eq!(
            profile.prompt.as_deref(),
            Some("You are in planning mode. Do not modify files.")
        );
    }

    #[test]
    fn empty_agents_blocks_spawn_in_effective_tool_filter() {
        let profile = Profile {
            name: "architect".into(),
            tools: ToolFilter::Deny(BTreeSet::from(["write".into()])),
            agents: Some(Vec::new()),
            ..Profile::default()
        };
        let filter = profile.effective_tool_filter();
        assert!(!filter.allows("write"));
        assert!(!filter.allows("spawn"));
        assert!(filter.allows("read"));
    }

    #[test]
    fn skill_glob_matching_supports_exact_prefix_suffix_and_star() {
        let allow = vec![
            "plan-*".to_owned(),
            "design".to_owned(),
            "*-audit".to_owned(),
        ];
        assert!(skill_allowed(Some(&allow), "plan-review"));
        assert!(skill_allowed(Some(&allow), "design"));
        assert!(skill_allowed(Some(&allow), "security-audit"));
        assert!(!skill_allowed(Some(&allow), "deploy"));
        assert!(skill_allowed(None, "anything"));
    }

    #[test]
    fn loads_markdown_profiles_from_directory() {
        let dir = magi_model::scratch::Scratch::new("magi-profiles", "load-dir");
        std::fs::write(
            dir.join("architect.md"),
            "---\nmodel: m/a\nmode: ask\nagents: []\n---\nThink deeply.\n",
        )
        .expect("write");
        std::fs::write(dir.join("notes.txt"), "ignored").expect("write");
        let loaded = load_dir(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "architect");
        assert_eq!(loaded[0].agents, Some(Vec::new()));
    }
}
