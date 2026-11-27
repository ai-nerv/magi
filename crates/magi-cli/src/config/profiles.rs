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

/// All configured profiles, merging acknowledged pack profiles < `~/.config/magi/profiles/*.md`
/// < `magi.profiles` in `init.lua`.
#[must_use]
pub fn all(loaded: &Loaded) -> Vec<Profile> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let roots = magi_lua::plugins::Roots::discovered(&cwd);
    let pack_files = super::discovered::acknowledged_pack_profiles(&roots);
    let dir = super::config_dir().map(|d| d.join("profiles"));
    all_with_packs(loaded, &pack_files, dir.as_deref())
}

/// Same as [`all`], with an explicit drop-in directory (used by tests).
#[cfg(test)]
#[must_use]
pub fn all_with_dir(loaded: &Loaded, dir: Option<&Path>) -> Vec<Profile> {
    all_with_packs(loaded, &[], dir)
}

/// Same as [`all`], with explicit pack `.md` files and user drop-in directory.
#[must_use]
pub fn all_with_packs(loaded: &Loaded, pack_files: &[PathBuf], dir: Option<&Path>) -> Vec<Profile> {
    let mut merged: BTreeMap<String, Profile> = BTreeMap::new();
    for path in pack_files {
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if let Ok(text) = std::fs::read_to_string(path) {
            match magi_host::profiles::parse_markdown(name, &text) {
                Ok(profile) => {
                    merged.insert(profile.name.clone(), profile);
                }
                Err(why) => eprintln!("magi: skipped pack profile {}: {why}", path.display()),
            }
        }
    }
    if let Some(dir) = dir {
        for profile in load_dir(dir) {
            merged.insert(profile.name.clone(), profile);
        }
    }
    if let Some(table) = loaded.config.get("profiles").and_then(Value::as_object) {
        for (name, value) in table {
            if magi_host::profiles::is_reserved_name(name) {
                eprintln!("magi: skipped `magi.profiles.{name}`: reserved profile name");
                continue;
            }
            let Some(held) = value.as_object() else {
                continue;
            };
            merged.insert(name.clone(), parse_lua_profile(name, held));
        }
    }
    merged.into_values().collect()
}

fn parse_lua_profile(name: &str, held: &serde_json::Map<String, Value>) -> Profile {
    const KNOWN_KEYS: &[&str] = &[
        "description",
        "prompt",
        "model",
        "thinking",
        "models",
        "skills",
        "permissions",
        "tools",
        "agents",
    ];
    for key in held.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            eprintln!("magi: profile {name:?} has unknown key {key:?} (ignored)");
        }
    }
    let text = |key: &str| {
        held.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    let mut model = text("model");
    let mut thinking = text("thinking");
    let helpers = parse_models(name, held.get("models"), &mut model, &mut thinking);
    let (skills_preload, skills_allow) = parse_skills(held.get("skills"));
    let (mode, allow, ask, deny) = parse_permissions(name, held.get("permissions"));
    let tools = parse_tool_filter(held.get("tools"));
    let agents = held.get("agents").map(|v| parse_string_list(Some(v)));
    let prompt = parse_prompt(name, held.get("prompt"));

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

pub(super) fn parse_models(
    owner: &str,
    value: Option<&Value>,
    model: &mut Option<String>,
    thinking: &mut Option<String>,
) -> BTreeMap<String, String> {
    let mut helpers = BTreeMap::new();
    let Some(models) = value.and_then(Value::as_object) else {
        return helpers;
    };
    for (k, v) in models {
        if v.as_bool() == Some(false) && !matches!(k.as_str(), "main" | "thinking") {
            helpers.insert(k.clone(), String::new());
            continue;
        }
        let Some(val) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            eprintln!("magi: {owner:?} models.{k} expects a model string (or false for helpers)");
            continue;
        };
        match k.as_str() {
            "main" => {
                if model.is_none() {
                    *model = Some(val.to_owned());
                }
            }
            "thinking" => {
                if thinking.is_none() {
                    *thinking = Some(val.to_owned());
                }
            }
            role => {
                helpers.insert(role.to_owned(), val.to_owned());
            }
        }
    }
    helpers
}

/// Drop-in `.md` profile files in `dir`, for config staleness detection.
pub(super) fn md_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    out.sort();
    out
}

/// Post-process a constructed catalog: warn if the active profile's requested model is unavailable,
/// and clamp any child agent role's requested thinking level to what the chosen model supports.
pub(super) fn finish_catalog(
    catalog: &mut magi_host::catalog::Catalog,
    role_thinking: Option<magi_model::ThinkingLevel>,
) {
    if let Some(profile) = catalog.active_profile_ref()
        && let Some(model) = profile.model.as_deref()
        && catalog.backend(model).is_none()
    {
        let why = catalog
            .unusable(model)
            .unwrap_or_else(|| format!("model {model:?} is not available"));
        eprintln!(
            "magi: profile {:?} requested {model:?}, falling back ({why})",
            profile.name
        );
    }
    if let Some(requested) = role_thinking
        && let Some(chosen) = catalog.chosen.as_deref()
        && let Some(supported) = catalog.thinking_levels(chosen)
    {
        catalog.wants.thinking = Some(magi_host::catalog::clamp_thinking(requested, &supported));
    }
}

pub(super) fn parse_prompt(owner: &str, value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        Value::Object(map) => {
            let raw = map.get("file")?.as_str()?.trim();
            let path = expand_tilde(raw);
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(why) => {
                    eprintln!("magi: {owner:?} prompt file {}: {why}", path.display());
                    return None;
                }
            };
            if !meta.is_file() || meta.len() > PROMPT_FILE_MOST {
                eprintln!(
                    "magi: {owner:?} prompt file {} is not a regular file within 64 KiB",
                    path.display()
                );
                return None;
            }
            let contents = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(why) => {
                    eprintln!("magi: {owner:?} prompt file {}: {why}", path.display());
                    return None;
                }
            };
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
    let path = PathBuf::from(raw);
    if path.is_relative()
        && let Some(dir) = super::config_dir()
    {
        return dir.join(path);
    }
    path
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
    owner: &str,
    value: Option<&Value>,
) -> (Option<Mode>, Vec<Grant>, Vec<Grant>, Vec<Grant>) {
    let Some(map) = value.and_then(Value::as_object) else {
        return (None, Vec::new(), Vec::new(), Vec::new());
    };
    let mode = map.get("mode").and_then(Value::as_str).and_then(|raw| {
        let parsed = Mode::named(raw);
        if parsed.is_none() {
            eprintln!("magi: {owner:?} has unknown permission mode {raw:?} (ignored)");
        }
        parsed
    });
    let allow = parse_grant_list(owner, "allow", map.get("allow"));
    let ask = parse_grant_list(owner, "ask", map.get("ask"));
    let deny = parse_grant_list(owner, "deny", map.get("deny"));
    (mode, allow, ask, deny)
}

pub(super) fn parse_tool_filter(value: Option<&Value>) -> ToolFilter {
    let Some(map) = value.and_then(Value::as_object) else {
        return ToolFilter::All;
    };
    match (map.get("allow"), map.get("deny")) {
        (Some(allow), Some(deny)) => {
            let mut allowed: BTreeSet<String> =
                parse_string_list(Some(allow)).into_iter().collect();
            for blocked in parse_string_list(Some(deny)) {
                allowed.remove(&blocked);
            }
            ToolFilter::Allow(allowed)
        }
        (Some(allow), None) => {
            ToolFilter::Allow(parse_string_list(Some(allow)).into_iter().collect())
        }
        (None, Some(deny)) => ToolFilter::Deny(parse_string_list(Some(deny)).into_iter().collect()),
        (None, None) => ToolFilter::All,
    }
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

pub(super) fn parse_grant_list(owner: &str, field: &str, value: Option<&Value>) -> Vec<Grant> {
    let Some(value) = value else {
        return Vec::new();
    };
    let single;
    let items: &[Value] = match value {
        Value::Array(items) => items,
        Value::Object(map) if map.is_empty() => return Vec::new(),
        other @ (Value::String(_) | Value::Object(_)) => {
            eprintln!(
                "magi: {owner:?} permissions.{field} should be a list; wrapping single entry"
            );
            single = [other.clone()];
            &single
        }
        other => {
            eprintln!("magi: {owner:?} permissions.{field} expects a list, got {other} (ignored)");
            return Vec::new();
        }
    };
    items
        .iter()
        .filter_map(|item| {
            let parsed = if let Some(shorthand) = item.as_str() {
                parse_grant_shorthand(shorthand)
            } else {
                let verb = item.get("verb").and_then(Value::as_str)?;
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
                    eprintln!(
                        "magi: {owner:?} permissions.{field} entry {item} has no scope (ignored)"
                    );
                    return None;
                };
                Some(Grant {
                    verb: verb.to_owned(),
                    scope,
                })
            };
            if parsed.is_none() {
                eprintln!("magi: {owner:?} permissions.{field} entry {item} is invalid (ignored)");
            }
            parsed
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
        app.overlay = None;

        // Switching sessions via SessionSnapshot must clear stale profile state and picker.
        app.open_profile_picker();
        assert!(app.overlay.is_some());
        app.apply(magi_proto::HarnessEvent::SessionSnapshot {
            cursor: magi_proto::Cursor(2),
            session: magi_proto::SessionId::new("child"),
            entries: Vec::new(),
            status: magi_proto::AgentStatus::Idle,
            model: None,
            choices: Vec::new(),
            thinking: "off".into(),
            judging: Default::default(),
        });
        assert_eq!(app.profile, None);
        assert!(app.profiles.is_empty());
        assert!(app.overlay.is_none());
        assert_eq!(app.picking, None);

        // View-only refuses opening the profile picker, and attach_to clears stale profile state.
        app.apply(magi_proto::HarnessEvent::ProfileChanged {
            cursor: magi_proto::Cursor(3),
            profile: Some("architect".into()),
            profiles: vec![("architect".into(), "design".into())],
        });
        app.view_only = true;
        app.open_profile_picker();
        assert!(app.overlay.is_none());
        app.attach_to(None);
        assert_eq!(app.profile, None);
        assert!(app.profiles.is_empty());
    }

    #[test]
    fn helper_disabling_md_watcher_and_child_thinking_clamp() {
        let cfg = loaded(
            r#"
            magi.profiles = {
              quiet = {
                models = { memory = false, safety = "m/safe" },
              },
            }
            "#,
        );
        let profiles = all_with_dir(&cfg, None);
        assert_eq!(profiles.len(), 1);
        let mut catalog = magi_host::catalog::Catalog::empty();
        catalog
            .helpers
            .roles
            .insert("memory".into(), "@main".into());
        catalog.profiles = profiles.clone();
        catalog.active_profile = Some("quiet".into());
        let eff = catalog.effective_helpers();
        assert!(!eff.roles.contains_key("memory"));
        assert_eq!(eff.roles.get("safety").map(String::as_str), Some("m/safe"));

        // Markdown profiles disable helpers via `memory: false` and are found by `md_files`.
        let dir = magi_model::scratch::Scratch::new("magi-profiles", "md-watch");
        std::fs::write(dir.join("quiet.md"), "---\nmemory: false\n---\nQuiet.\n").expect("write");
        std::fs::write(dir.join("ignore.txt"), "skip").expect("write");
        assert_eq!(md_files(&dir), vec![dir.join("quiet.md")]);
        let md_loaded = magi_host::profiles::load_dir(&dir);
        assert_eq!(
            md_loaded[0].helpers.get("memory").map(String::as_str),
            Some("")
        );

        // finish_catalog clamps child role thinking to what the chosen model supports.
        let mut card = serde_json::from_value::<magi_proto::ask::Card>(serde_json::json!({
            "id": "m/plain", "provider": "m", "name": "plain", "api": "x", "ready": true
        }))
        .expect("card");
        card.reasons = false;
        catalog.cards.push(card);
        catalog.chosen = Some("m/plain".into());
        catalog.active_profile = None;
        finish_catalog(&mut catalog, Some(magi_model::ThinkingLevel::Max));
        assert_eq!(catalog.wants.thinking, Some(magi_model::ThinkingLevel::Off));
    }
}
