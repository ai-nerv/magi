//! Inline `/command` registration and dispatch (`magi.command(name, spec)`).
//!
//! Unlike colon commands (`:model`, `:profile`), which live on the vim command line, inline
//! `/command` tokens work anywhere inside a prompt in insert mode (e.g. `"/goal fix tests"`,
//! `"refactor parser /goal"`, `"/goal stop"`). Registered handlers run in the session's sandboxed
//! Lua VM and can rewrite the prompt, attach turn instructions (`aside`), emit notices, mutate
//! plugin-local state (for turn hooks like `before_turn`/`after_turn`), or handle the input
//! locally without calling the model (`handled = true`).

use crate::Engine;
use crate::convert::{json_from_lua, lua_from_json};
use luna::{Callback, CallbackReturn, Context, Table, Value};
use std::collections::BTreeSet;

pub(crate) const COMMANDS: &str = "__magi_commands";

/// Input handed to inline `/command` dispatch.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub prompt: &'a str,
    pub aside: &'a str,
    pub profile: Option<&'a str>,
    pub model: Option<&'a str>,
}

/// Result of running all matched inline `/command` tokens in a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Whether at least one registered `/command` matched.
    pub matched: bool,
    /// When true, the command handled the input locally and no model turn should run.
    pub handled: bool,
    pub prompt: String,
    pub aside: String,
    pub notices: Vec<String>,
}

/// Install `magi.command(name, spec)` onto the `magi` global table.
pub(crate) fn install<'gc>(ctx: Context<'gc>, magi: Table<'gc>) {
    let table = Table::new(&ctx);
    ctx.set_global(COMMANDS, table);
    let register = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        let (name, spec): (Value, Value) = stack.consume(ctx)?;
        let (Value::String(name), Value::Table(_)) = (name, spec) else {
            return Err(luna::Error::from_value(
                luna::String::from_slice(&ctx, b"magi.command(name, spec): a name and a table")
                    .into(),
            ));
        };
        let raw = String::from_utf8_lossy(name.as_bytes());
        let trimmed = raw.trim();
        if !valid_name(trimmed) {
            let msg = format!(
                "magi.command: invalid name {trimmed:?} (use lowercase letters/digits/-/_ and not `skill`)"
            );
            return Err(luna::Error::from_value(
                luna::String::from_slice(&ctx, msg.as_bytes()).into(),
            ));
        }
        if let Value::Table(commands) = ctx.get_global_value(COMMANDS) {
            commands.set(ctx, trimmed, spec).ok();
        }
        stack.replace(ctx, ());
        Ok(CallbackReturn::Return)
    });
    magi.set(ctx, "command", register).ok();
}

/// Valid inline command name: lowercase ASCII identifier, never shadowing `/skill`.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    if name.is_empty() || name == "skill" || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// Whether at least one inline `/command` is registered in this VM.
pub fn has_any(engine: &mut Engine) -> bool {
    let mut any = false;
    engine.enter(|ctx| {
        if let Value::Table(commands) = ctx.get_global_value(COMMANDS) {
            any = commands.iter(ctx).next().is_some();
        }
    });
    any
}

/// Sorted `(name, description)` list of every registered inline `/command`.
pub fn list(engine: &mut Engine) -> Vec<(String, String)> {
    let mut out = Vec::new();
    engine.enter(|ctx| {
        let Value::Table(commands) = ctx.get_global_value(COMMANDS) else {
            return;
        };
        for (k, v) in commands.iter(ctx) {
            let Value::String(name) = k else { continue };
            let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
            let desc = match v {
                Value::Table(spec) => match spec.get_value(ctx, "description") {
                    Value::String(s) => String::from_utf8_lossy(s.as_bytes()).trim().to_owned(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            out.push((name, desc));
        }
    });
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// One matched `/name` token inside a prompt string.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    name: String,
    start: usize,
    end: usize,
}

/// Scan `text` for word-bounded `/name` tokens matching any registered command name.
fn scan(text: &str, known: &BTreeSet<String>) -> Vec<Found> {
    if known.is_empty() || !text.contains('/') {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = text.get(i..).and_then(|s| s.chars().next()).unwrap_or('\0');
        let prev_ws = i == 0
            || text
                .get(..i)
                .and_then(|s| s.chars().next_back())
                .is_some_and(char::is_whitespace);
        if ch == '/' && prev_ws {
            let name_start = i + 1;
            let mut end = name_start;
            while let Some(c) = text.get(end..).and_then(|s| s.chars().next()) {
                if c.is_whitespace() {
                    break;
                }
                end += c.len_utf8();
            }
            if let Some(candidate) = text.get(name_start..end)
                && known.contains(candidate)
            {
                out.push(Found {
                    name: candidate.to_owned(),
                    start: i,
                    end,
                });
            }
            i = end;
        } else {
            i += ch.len_utf8();
        }
    }
    out
}

/// Remove a matched `/name` token at `[start..end]` while collapsing surrounding whitespace cleanly.
fn strip_token(text: &str, start: usize, end: usize) -> String {
    let before = text.get(..start).unwrap_or("").trim_end();
    let after = text.get(end..).unwrap_or("").trim_start();
    match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.to_owned(),
        (false, true) => before.to_owned(),
        (false, false) => format!("{before} {after}"),
    }
}

/// Dispatch any registered inline `/command` tokens found anywhere in `req.prompt`.
pub fn dispatch(engine: &mut Engine, req: &Request<'_>) -> Outcome {
    let known: BTreeSet<String> = list(engine).into_iter().map(|(name, _)| name).collect();
    if known.is_empty() {
        return Outcome {
            matched: false,
            handled: false,
            prompt: req.prompt.to_owned(),
            aside: req.aside.to_owned(),
            notices: Vec::new(),
        };
    }
    let initial = scan(req.prompt, &known);
    if initial.is_empty() {
        return Outcome {
            matched: false,
            handled: false,
            prompt: req.prompt.to_owned(),
            aside: req.aside.to_owned(),
            notices: Vec::new(),
        };
    }

    let mut prompt = req.prompt.to_owned();
    let mut aside = req.aside.to_owned();
    let mut notices = Vec::new();
    let mut handled = false;
    let mut ran: BTreeSet<String> = BTreeSet::new();

    // Re-scan after each command so earlier prompt rewrites stay consistent, guarding against loops.
    for _ in 0..initial.len().min(16) {
        let hits = scan(&prompt, &known);
        let Some(hit) = hits.iter().find(|h| !ran.contains(&h.name)) else {
            break;
        };
        ran.insert(hit.name.clone());
        let next_start = hits
            .iter()
            .find(|other| other.start > hit.end)
            .map_or(prompt.len(), |other| other.start);
        let args = prompt.get(hit.end..next_start).unwrap_or("").trim();
        let cleaned = strip_token(&prompt, hit.start, hit.end);

        let payload = serde_json::json!({
            "name": hit.name,
            "args": args,
            "prompt": prompt,
            "cleaned": cleaned,
            "aside": aside,
            "profile": req.profile,
            "model": req.model,
        });
        let result = call_one(engine, &hit.name, &payload);
        if let Some(res) = result.as_object() {
            if let Some(notice) = res
                .get("notice")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                notices.push(notice.to_owned());
            }
            if let Some(extra) = res
                .get("aside")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if !aside.is_empty() {
                    aside.push_str("\n\n");
                }
                aside.push_str(extra);
            }
            if res.get("handled").and_then(serde_json::Value::as_bool) == Some(true) {
                handled = true;
                break;
            }
            if let Some(rewritten) = res.get("prompt").and_then(serde_json::Value::as_str) {
                prompt = rewritten.to_owned();
            } else if res.get("keep_token").and_then(serde_json::Value::as_bool) != Some(true) {
                prompt = cleaned;
            }
        } else {
            prompt = cleaned;
        }
    }

    Outcome {
        matched: true,
        handled,
        prompt,
        aside,
        notices,
    }
}

fn call_one(engine: &mut Engine, name: &str, ctx_json: &serde_json::Value) -> serde_json::Value {
    engine.enter(|ctx| {
        ctx.set_global(
            "__magi_cmd_name",
            luna::String::from_slice(&ctx, name.as_bytes()),
        );
        ctx.set_global("__magi_cmd_ctx", lua_from_json(ctx, ctx_json));
        ctx.set_global("__magi_cmd_result", Value::Nil);
    });
    let source = format!(
        "local spec = {COMMANDS}[__magi_cmd_name]\n\
         if type(spec) == 'table' and type(spec.run) == 'function' then\n\
           local ok, res = pcall(spec.run, __magi_cmd_ctx)\n\
           if ok and type(res) == 'table' then\n\
             __magi_cmd_result = res\n\
           elseif not ok then\n\
             __magi_cmd_result = {{ notice = 'command /' .. __magi_cmd_name .. ' failed: ' .. tostring(res) }}\n\
           end\n\
         end"
    );
    if let Err(why) = engine.run_call_budget(&source, "command_dispatch.lua") {
        return serde_json::json!({ "notice": format!("command /{name} stopped: {why}") });
    }
    let mut out = serde_json::Value::Null;
    engine.enter(|ctx| {
        out = json_from_lua(ctx, ctx.get_global_value("__magi_cmd_result"))
            .unwrap_or(serde_json::Value::Null);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_command_matches_anywhere_and_ignores_paths_and_skill() {
        let mut engine = Engine::new();
        engine
            .run(
                r#"
                magi.command("goal", {
                  description = "Activate goal mode",
                  run = function(ctx)
                    if ctx.args == "stop" then
                      return { handled = true, notice = "stopped" }
                    end
                    return {
                      aside = "Goal active: " .. ctx.cleaned,
                      notice = "goal started",
                    }
                  end,
                })
                "#,
                "test.lua",
            )
            .expect("register");

        assert_eq!(
            list(&mut engine),
            vec![("goal".to_owned(), "Activate goal mode".to_owned())]
        );

        // Mid-sentence trigger works; path slashes do not match.
        let out = dispatch(
            &mut engine,
            &Request {
                prompt: "fix src/goal/mod.rs /goal and verify tests",
                aside: "",
                profile: Some("executor"),
                model: Some("m/a"),
            },
        );
        assert!(out.matched);
        assert!(!out.handled);
        assert_eq!(out.prompt, "fix src/goal/mod.rs and verify tests");
        assert_eq!(
            out.aside,
            "Goal active: fix src/goal/mod.rs and verify tests"
        );
        assert_eq!(out.notices, vec!["goal started"]);

        // Local-only handled command.
        let stop = dispatch(
            &mut engine,
            &Request {
                prompt: "/goal stop",
                aside: "",
                profile: None,
                model: None,
            },
        );
        assert!(stop.matched);
        assert!(stop.handled);
        assert_eq!(stop.notices, vec!["stopped"]);

        // `/skill` is never intercepted or registerable.
        assert!(!valid_name("skill"));
        let path_only = dispatch(
            &mut engine,
            &Request {
                prompt: "read /tmp/goal/file.txt",
                aside: "",
                profile: None,
                model: None,
            },
        );
        assert!(!path_only.matched);
    }
}
