//! Higher-level session profiles from `magi.profiles` and `~/.config/magi/profiles/*.md`.
//!
//! A profile configures how your top-level session operates: prompt, models/thinking, skills,
//! permission overlays, tool filters, and which subagents (`magi.agents`) it may spawn. Lua
//! entries in `magi.profiles` override drop-in Markdown files of the same name.

use super::Loaded;
use magi_host::profiles::{Profile, load_dir, parse_grant_shorthand};
use magi_proto::judging::Mode;
use magi_proto::permit::{Grant, Scope};
use magi_tools::ToolFilter;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const PROMPT_FILE_MOST: u64 = 64 * 1024;

/// Optional default profile named by `magi.default_profile`.
#[must_use]
pub fn default_name(loaded: &Loaded) -> Option<String> {
    loaded
        .config
        .string("default_profile")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// All configured profiles, merging `~/.config/magi/profiles/*.md` underneath `magi.profiles`.
#[must_use]
pub fn all(loaded: &Loaded) -> Vec<Profile> {
    let dir = super::config_dir().map(|d| d.join("profiles"));
    all_with_dir(loaded, dir.as_deref())
}

/// Same as [`all`], with an explicit drop-in directory (used by tests).
#[must_use]
pub fn all_with_dir(loaded: &Loaded, dir: Option<&Path>) -> Vec<Profile> {
    let mut merged: BTreeMap<String, Profile> = BTreeMap::new();
    if let Some(dir) = dir {
        for profile in load_dir(dir) {
            merged.insert(profile.name.clone(), profile);
        }
    }
    if let Some(table) = loaded.config.get("profiles").and_then(Value::as_object) {
        for (name, value) in table {
            let Some(held) = value.as_object() else {
                continue;
            };
            merged.insert(name.clone(), parse_lua_profile(name, held));
        }
    }
    merged.into_values().collect()
}

fn parse_lua_profile(name: &str, held: &serde_json::Map<String, Value>) -> Profile {
    let text = |key: &str| {
        held.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    let mut model = text("model");
    let mut thinking = text("thinking");
    let mut helpers = BTreeMap::new();
    if let Some(models) = held.get("models").and_then(Value::as_object) {
        for (k, v) in models {
            let Some(val) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            match k.as_str() {
                "main" => {
                    if model.is_none() {
                        model = Some(val.to_owned());
                    }
                }
                "thinking" => {
                    if thinking.is_none() {
                        thinking = Some(val.to_owned());
                    }
                }
                role => {
                    helpers.insert(role.to_owned(), val.to_owned());
                }
            }
        }
    }
    let (skills_preload, skills_allow) = parse_skills(held.get("skills"));
    let (mode, allow, ask, deny) = parse_permissions(held.get("permissions"));
    let tools = parse_tool_filter(held.get("tools"));
    let agents = held.get("agents").map(|v| parse_string_list(Some(v)));
    let prompt = parse_prompt(held.get("prompt"));

    Profile {
        name: name.to_owned(),
        description: text("description"),
        prompt,
        model,
        thinking,
        helpers,
        skills_preload,
        skills_allow,
        mode,
        allow,
        ask,
        deny,
        tools,
        agents,
    }
}

pub(super) fn parse_prompt(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        Value::Object(map) => {
            let raw = map.get("file")?.as_str()?.trim();
            let path = expand_tilde(raw);
            let meta = std::fs::metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > PROMPT_FILE_MOST {
                return None;
            }
            let contents = std::fs::read_to_string(&path).ok()?;
            let trimmed = contents.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        _ => None,
    }
}

fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty())
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(raw)
}

pub(super) fn parse_skills(value: Option<&Value>) -> (Vec<String>, Option<Vec<String>>) {
    let Some(map) = value.and_then(Value::as_object) else {
        return (Vec::new(), None);
    };
    let preload = parse_string_list(map.get("preload"));
    let allow = map.get("allow").map(|v| parse_string_list(Some(v)));
    (preload, allow)
}

pub(super) fn parse_permissions(
    value: Option<&Value>,
) -> (Option<Mode>, Vec<Grant>, Vec<Grant>, Vec<Grant>) {
    let Some(map) = value.and_then(Value::as_object) else {
        return (None, Vec::new(), Vec::new(), Vec::new());
    };
    let mode = map
        .get("mode")
        .and_then(Value::as_str)
        .and_then(Mode::named);
    let allow = parse_grant_list(map.get("allow"));
    let ask = parse_grant_list(map.get("ask"));
    let deny = parse_grant_list(map.get("deny"));
    (mode, allow, ask, deny)
}

pub(super) fn parse_tool_filter(value: Option<&Value>) -> ToolFilter {
    let Some(map) = value.and_then(Value::as_object) else {
        return ToolFilter::All;
    };
    if let Some(allow) = map.get("allow") {
        let set: BTreeSet<String> = parse_string_list(Some(allow)).into_iter().collect();
        return ToolFilter::Allow(set);
    }
    if let Some(deny) = map.get("deny") {
        let set: BTreeSet<String> = parse_string_list(Some(deny)).into_iter().collect();
        return ToolFilter::Deny(set);
    }
    ToolFilter::All
}

pub(super) fn parse_string_list(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        // Lua empty table `{}` serialises as an empty JSON object.
        Value::Object(map) if map.is_empty() => Vec::new(),
        _ => Vec::new(),
    }
}

pub(super) fn parse_grant_list(value: Option<&Value>) -> Vec<Grant> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if let Some(shorthand) = item.as_str() {
                return parse_grant_shorthand(shorthand);
            }
            let verb = item.get("verb")?.as_str()?.to_owned();
            let scope = if item.get("anything").and_then(Value::as_bool) == Some(true) {
                Scope::Anything
            } else if let Some(program) = item.get("program").and_then(Value::as_str) {
                Scope::Program {
                    program: program.to_owned(),
                }
            } else if let Some(path) = item.get("directory").and_then(Value::as_str) {
                Scope::Directory {
                    path: path.to_owned(),
                }
            } else {
                return None;
            };
            Some(Grant { verb, scope })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(config: &str) -> Loaded {
        let mut engine = magi_lua::Engine::new();
        engine.run(config, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
        }
    }

    #[test]
    fn parses_lua_profiles_and_merges_over_markdown_dropins() {
        let dir = magi_model::scratch::Scratch::new("magi-profiles", "merge");
        std::fs::write(
            dir.join("planner.md"),
            "---\nmodel: md/old\nmode: ask\n---\nFrom markdown.\n",
        )
        .expect("write");
        std::fs::write(
            dir.join("architect.md"),
            "---\nmodel: anthropic/opus\nthinking: max\nagents: []\n---\nThink deeply.\n",
        )
        .expect("write");

        let cfg = loaded(
            r#"
            magi.default_profile = "planner"
            magi.profiles = {
              planner = {
                description = "Plans only",
                prompt = "Plan cleanly.",
                model = "anthropic/claude-opus-4-6",
                thinking = "high",
                models = { safety = "anthropic/haiku" },
                skills = { preload = { "plan-review" }, allow = { "plan-*" } },
                permissions = {
                  mode = "ask",
                  allow = { "read", { verb = "run", program = "git" } },
                  deny = { "write", "run:rm" },
                },
                tools = { deny = { "write", "edit" } },
                agents = { "explorer" },
              },
            }
            "#,
        );
        assert_eq!(default_name(&cfg).as_deref(), Some("planner"));
        let profiles = all_with_dir(&cfg, Some(&dir));
        assert_eq!(profiles.len(), 2);

        let arch = profiles
            .iter()
            .find(|p| p.name == "architect")
            .expect("arch");
        assert_eq!(arch.agents, Some(Vec::new()));
        assert!(!arch.effective_tool_filter().allows("spawn"));

        let planner = profiles
            .iter()
            .find(|p| p.name == "planner")
            .expect("planner");
        assert_eq!(planner.model.as_deref(), Some("anthropic/claude-opus-4-6"));
        assert_eq!(planner.thinking.as_deref(), Some("high"));
        assert_eq!(
            planner.helpers.get("safety").map(String::as_str),
            Some("anthropic/haiku")
        );
        assert_eq!(planner.allow.len(), 2);
        assert_eq!(planner.deny.len(), 2);
        assert_eq!(planner.skills_preload, vec!["plan-review"]);
        assert_eq!(planner.skills_allow, Some(vec!["plan-*".to_owned()]));
        assert_eq!(planner.agents, Some(vec!["explorer".to_owned()]));
        assert!(!planner.effective_tool_filter().allows("write"));
        assert!(planner.effective_tool_filter().allows("spawn"));
    }

    #[test]
    fn untrusted_project_file_cannot_set_profiles_or_default_profile() {
        let dir = magi_model::scratch::Scratch::new("magi-profiles", "untrusted");
        let path = dir.join(".magi.lua");
        std::fs::write(&path, r#"magi.profiles = { p = { model = "x" } }"#).expect("write");
        let mut engine = magi_lua::Engine::new();
        let err = crate::config::project::apply(&mut engine, &path).expect_err("refused");
        assert!(err.to_string().contains("magi.profiles"), "{err}");

        std::fs::write(&path, r#"magi.default_profile = "p""#).expect("write");
        let err = crate::config::project::apply(&mut engine, &path).expect_err("refused");
        assert!(err.to_string().contains("magi.default_profile"), "{err}");
    }

    #[test]
    fn tui_profile_picker_and_cycle_follow_available_profiles() {
        let mut app = crate::app::App::new();
        app.apply(magi_proto::HarnessEvent::ProfileChanged {
            cursor: magi_proto::Cursor(1),
            profile: None,
            profiles: vec![
                ("architect".into(), "design".into()),
                ("executor".into(), "build".into()),
            ],
        });
        assert_eq!(app.next_profile().as_deref(), Some("architect"));
        app.profile = Some("architect".into());
        assert_eq!(app.next_profile().as_deref(), Some("executor"));
        app.profile = Some("executor".into());
        assert_eq!(app.next_profile().as_deref(), Some("default"));

        app.open_profile_picker();
        assert_eq!(app.picking, Some(crate::app::Picking::Profile));
        assert_eq!(
            app.chose("architect".into()),
            Some(magi_proto::UiCommand::SetProfile {
                name: "architect".into()
            })
        );
    }
}
