//! Pre/post tool hooks registered in Lua via `magi.hook(name, spec)`.

use crate::Engine;
use crate::convert::{json_from_lua, lua_from_json};
use magi_tools::hooking::{After, Before, Hook};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) const HOOKS: &str = "__magi_hooks";

/// Context passed to `before_turn(ctx)` hooks right before a model turn runs.
#[derive(Debug, Clone)]
pub struct TurnBefore<'a> {
    pub model: &'a str,
    pub profile: Option<&'a str>,
    pub iteration: usize,
    pub prompt: Option<&'a str>,
}

/// Output collected from `before_turn(ctx)` hooks for one turn.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TurnBeforeOut {
    pub system: Vec<String>,
    pub notices: Vec<String>,
}

/// Context passed to `after_turn(ctx)` hooks right after a model turn finishes.
#[derive(Debug, Clone)]
pub struct TurnAfter<'a> {
    pub model: &'a str,
    pub profile: Option<&'a str>,
    pub iteration: usize,
    pub text: &'a str,
    pub ok: bool,
    pub cancelled: bool,
}

/// Output collected from `after_turn(ctx)` hooks after one turn.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TurnAfterOut {
    /// Optional `(prompt, aside)` follow-up turn to continue an autonomous loop.
    pub continue_prompt: Option<(String, String)>,
    pub notices: Vec<String>,
}

/// Whether at least one `magi.hook` entry is registered in this VM.
pub fn has_any(engine: &mut Engine) -> bool {
    let mut any = false;
    engine.enter(|ctx| {
        if let luna::Value::Table(hooks) = ctx.get_global_value(HOOKS) {
            any = hooks.iter(ctx).next().is_some();
        }
    });
    any
}

/// Run all `magi.hook` `before_turn(ctx)` callbacks in sorted name order.
pub fn before_turn(engine: &mut Engine, info: &TurnBefore<'_>) -> TurnBeforeOut {
    if !has_any(engine) {
        return TurnBeforeOut::default();
    }
    let payload = serde_json::json!({
        "model": info.model,
        "profile": info.profile,
        "iteration": info.iteration,
        "prompt": info.prompt,
    });
    engine.enter(|ctx| {
        ctx.set_global("__magi_turn_ctx", lua_from_json(ctx, &payload));
        ctx.set_global("__magi_turn_result", luna::Value::Nil);
    });
    let source = format!(
        "local names = {{}}\n\
         for k in pairs({HOOKS}) do if type(k) == 'string' then names[#names + 1] = k end end\n\
         table.sort(names)\n\
         local systems, notices = {{}}, {{}}\n\
         for _, name in ipairs(names) do\n\
           local spec = {HOOKS}[name]\n\
           if type(spec) == 'table' and type(spec.before_turn) == 'function' then\n\
             local ok, res = pcall(spec.before_turn, __magi_turn_ctx)\n\
             if ok and type(res) == 'table' then\n\
               if type(res.system) == 'string' and res.system ~= '' then systems[#systems + 1] = res.system end\n\
               if type(res.notice) == 'string' and res.notice ~= '' then notices[#notices + 1] = res.notice end\n\
             elseif ok and type(res) == 'string' and res ~= '' then\n\
               systems[#systems + 1] = res\n\
             elseif not ok then\n\
               notices[#notices + 1] = 'hook ' .. name .. '.before_turn failed: ' .. tostring(res)\n\
             end\n\
           end\n\
         end\n\
         __magi_turn_result = {{ system = systems, notices = notices }}"
    );
    if let Err(why) = engine.run_call_budget(&source, "hook_before_turn.lua") {
        return TurnBeforeOut {
            system: Vec::new(),
            notices: vec![format!("before_turn hook stopped: {why}")],
        };
    }
    let mut raw = serde_json::Value::Null;
    engine.enter(|ctx| {
        raw = json_from_lua(ctx, ctx.get_global_value("__magi_turn_result"))
            .unwrap_or(serde_json::Value::Null);
    });
    let strings = |key: &str| -> Vec<String> {
        raw.get(key)
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    TurnBeforeOut {
        system: strings("system"),
        notices: strings("notices"),
    }
}

/// Run all `magi.hook` `after_turn(ctx)` callbacks in sorted name order.
pub fn after_turn(engine: &mut Engine, info: &TurnAfter<'_>) -> TurnAfterOut {
    if !has_any(engine) {
        return TurnAfterOut::default();
    }
    let payload = serde_json::json!({
        "model": info.model,
        "profile": info.profile,
        "iteration": info.iteration,
        "text": info.text,
        "ok": info.ok,
        "cancelled": info.cancelled,
    });
    engine.enter(|ctx| {
        ctx.set_global("__magi_turn_ctx", lua_from_json(ctx, &payload));
        ctx.set_global("__magi_turn_result", luna::Value::Nil);
    });
    let source = format!(
        "local names = {{}}\n\
         for k in pairs({HOOKS}) do if type(k) == 'string' then names[#names + 1] = k end end\n\
         table.sort(names)\n\
         local notices = {{}}\n\
         local cont_prompt, cont_aside = nil, ''\n\
         for _, name in ipairs(names) do\n\
           local spec = {HOOKS}[name]\n\
           if type(spec) == 'table' and type(spec.after_turn) == 'function' then\n\
             local ok, res = pcall(spec.after_turn, __magi_turn_ctx)\n\
             if ok and type(res) == 'table' then\n\
               if type(res.notice) == 'string' and res.notice ~= '' then notices[#notices + 1] = res.notice end\n\
               if cont_prompt == nil and type(res.continue) == 'string' and res.continue ~= '' then\n\
                 cont_prompt = res.continue\n\
                 if type(res.aside) == 'string' then cont_aside = res.aside end\n\
               end\n\
             elseif not ok then\n\
               notices[#notices + 1] = 'hook ' .. name .. '.after_turn failed: ' .. tostring(res)\n\
             end\n\
           end\n\
         end\n\
         __magi_turn_result = {{ notices = notices, continue = cont_prompt, aside = cont_aside }}"
    );
    if let Err(why) = engine.run_call_budget(&source, "hook_after_turn.lua") {
        return TurnAfterOut {
            continue_prompt: None,
            notices: vec![format!("after_turn hook stopped: {why}")],
        };
    }
    let mut raw = serde_json::Value::Null;
    engine.enter(|ctx| {
        raw = json_from_lua(ctx, ctx.get_global_value("__magi_turn_result"))
            .unwrap_or(serde_json::Value::Null);
    });
    let notices = raw
        .get("notices")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let continue_prompt = if info.cancelled {
        None
    } else {
        raw.get("continue")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|prompt| {
                let aside = raw
                    .get("aside")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_owned();
                (prompt.to_owned(), aside)
            })
    };
    TurnAfterOut {
        continue_prompt,
        notices,
    }
}

/// A Lua-backed tool hook bridging `magi.hook(name, spec)` into [`magi_tools::Registry`].
pub struct LuaHook {
    engine: Rc<RefCell<Engine>>,
}

impl LuaHook {
    #[must_use]
    pub fn new(engine: Rc<RefCell<Engine>>) -> Self {
        Self { engine }
    }
}

impl Hook for LuaHook {
    fn before(&self, tool: &str, arguments: &serde_json::Value) -> Before {
        let Ok(mut engine) = self.engine.try_borrow_mut() else {
            return Before::Continue;
        };
        call_before(&mut engine, tool, arguments)
    }

    fn after(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        content: &str,
        is_error: bool,
    ) -> Option<After> {
        let Ok(mut engine) = self.engine.try_borrow_mut() else {
            return None;
        };
        call_after(&mut engine, tool, arguments, content, is_error)
    }
}

fn call_before(engine: &mut Engine, tool: &str, arguments: &serde_json::Value) -> Before {
    let mut any = false;
    engine.enter(|ctx| {
        if let luna::Value::Table(hooks) = ctx.get_global_value(HOOKS) {
            any = hooks.iter(ctx).next().is_some();
        }
        if any {
            ctx.set_global(
                "__magi_hook_tool",
                luna::String::from_slice(&ctx, tool.as_bytes()),
            );
            ctx.set_global("__magi_hook_args", lua_from_json(ctx, arguments));
            ctx.set_global("__magi_hook_result", luna::Value::Nil);
        }
    });
    if !any {
        return Before::Continue;
    }
    let source = format!(
        "local names = {{}}\n\
         for k in pairs({HOOKS}) do if type(k) == 'string' then names[#names + 1] = k end end\n\
         table.sort(names)\n\
         local function matches(spec, tool)\n\
           if spec.tools == nil then return true end\n\
           if type(spec.tools) == 'string' then return spec.tools == tool end\n\
           if type(spec.tools) == 'table' then\n\
             for _, t in ipairs(spec.tools) do if t == tool then return true end end\n\
           end\n\
           return false\n\
         end\n\
         local current = __magi_hook_args\n\
         local changed = false\n\
         for _, name in ipairs(names) do\n\
           local spec = {HOOKS}[name]\n\
           if type(spec) == 'table' and type(spec.before) == 'function' and matches(spec, __magi_hook_tool) then\n\
             local ok, res = pcall(spec.before, __magi_hook_tool, current)\n\
             if ok and type(res) == 'table' then\n\
               if type(res.deny) == 'string' then\n\
                 __magi_hook_result = {{ deny = res.deny }}\n\
                 return\n\
               elseif res.args ~= nil then\n\
                 current = res.args\n\
                 changed = true\n\
               end\n\
             end\n\
           end\n\
         end\n\
         if changed then __magi_hook_result = {{ args = current }} end"
    );
    if engine.run_call_budget(&source, "hook_before.lua").is_err() {
        return Before::Continue;
    }
    let mut out = serde_json::Value::Null;
    engine.enter(|ctx| {
        out = json_from_lua(ctx, ctx.get_global_value("__magi_hook_result"))
            .unwrap_or(serde_json::Value::Null);
    });
    if let Some(reason) = out.get("deny").and_then(serde_json::Value::as_str) {
        return Before::Deny(reason.to_owned());
    }
    if let Some(rewritten) = out.get("args") {
        return Before::Rewrite(rewritten.clone());
    }
    Before::Continue
}

fn call_after(
    engine: &mut Engine,
    tool: &str,
    arguments: &serde_json::Value,
    content: &str,
    is_error: bool,
) -> Option<After> {
    let mut any = false;
    engine.enter(|ctx| {
        if let luna::Value::Table(hooks) = ctx.get_global_value(HOOKS) {
            any = hooks.iter(ctx).next().is_some();
        }
        if any {
            ctx.set_global(
                "__magi_hook_tool",
                luna::String::from_slice(&ctx, tool.as_bytes()),
            );
            ctx.set_global("__magi_hook_args", lua_from_json(ctx, arguments));
            ctx.set_global(
                "__magi_hook_out",
                lua_from_json(
                    ctx,
                    &serde_json::json!({ "content": content, "is_error": is_error }),
                ),
            );
            ctx.set_global("__magi_hook_result", luna::Value::Nil);
        }
    });
    if !any {
        return None;
    }
    let source = format!(
        "local names = {{}}\n\
         for k in pairs({HOOKS}) do if type(k) == 'string' then names[#names + 1] = k end end\n\
         table.sort(names)\n\
         local function matches(spec, tool)\n\
           if spec.tools == nil then return true end\n\
           if type(spec.tools) == 'string' then return spec.tools == tool end\n\
           if type(spec.tools) == 'table' then\n\
             for _, t in ipairs(spec.tools) do if t == tool then return true end end\n\
           end\n\
           return false\n\
         end\n\
         local current = __magi_hook_out\n\
         local changed = false\n\
         for _, name in ipairs(names) do\n\
           local spec = {HOOKS}[name]\n\
           if type(spec) == 'table' and type(spec.after) == 'function' and matches(spec, __magi_hook_tool) then\n\
             local ok, res = pcall(spec.after, __magi_hook_tool, __magi_hook_args, current)\n\
             if ok then\n\
               if type(res) == 'string' then\n\
                 current = {{ content = res, is_error = current.is_error }}\n\
                 changed = true\n\
               elseif type(res) == 'table' then\n\
                 if type(res.content) == 'string' then\n\
                   current.content = res.content\n\
                   changed = true\n\
                 end\n\
                 if type(res.is_error) == 'boolean' then\n\
                   current.is_error = res.is_error\n\
                   changed = true\n\
                 end\n\
               end\n\
             end\n\
           end\n\
         end\n\
         if changed then __magi_hook_result = current end"
    );
    if engine.run_call_budget(&source, "hook_after.lua").is_err() {
        return None;
    }
    let mut out = serde_json::Value::Null;
    engine.enter(|ctx| {
        out = json_from_lua(ctx, ctx.get_global_value("__magi_hook_result"))
            .unwrap_or(serde_json::Value::Null);
    });
    if out.is_null() {
        return None;
    }
    Some(After {
        content: out
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        is_error: out.get("is_error").and_then(serde_json::Value::as_bool),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_tools::{Cancel, Ops, Output, Registry, Tool, Uncancelled};
    use serde_json::json;

    struct Echo;
    impl Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes text"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            })
        }
        fn run(&self, arguments: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
            Output::ok(
                arguments
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            )
        }
    }

    #[test]
    fn lua_hooks_rewrite_deny_and_redact_outputs() {
        let mut engine = Engine::new();
        engine
            .run(
                r#"
                magi.hook("block-secret", {
                  tools = { "echo" },
                  before = function(tool, args)
                    if args.text == "forbidden" then
                      return { deny = "blocked by hook" }
                    end
                    return { args = { text = args.text .. " [rewritten]" } }
                  end,
                  after = function(tool, args, result)
                    return result.content:gsub("TOKEN123", "[REDACTED]")
                  end,
                })
                "#,
                "hooks.lua",
            )
            .expect("load hooks");
        let engine = Rc::new(RefCell::new(engine));
        let mut registry = Registry::new();
        registry.register(Box::new(Echo));
        registry.hook(Box::new(LuaHook::new(Rc::clone(&engine))));
        let ops = magi_tools::ops::Real::new(std::env::temp_dir());

        let denied = registry.call("echo", &json!({ "text": "forbidden" }), &ops, &Uncancelled);
        assert!(denied.is_error);
        assert!(
            denied.content.contains("blocked by hook"),
            "{}",
            denied.content
        );

        let rewritten = registry.call(
            "echo",
            &json!({ "text": "hello TOKEN123" }),
            &ops,
            &Uncancelled,
        );
        assert!(!rewritten.is_error);
        assert_eq!(rewritten.content, "hello [REDACTED] [rewritten]");

        let prepared = registry.answer("echo", r#"{"text":"via TOKEN123"}"#, &ops, &Uncancelled);
        assert!(!prepared.is_error);
        assert_eq!(prepared.content, "via [REDACTED] [rewritten]");
    }

    #[test]
    fn untrusted_project_file_cannot_register_hooks() {
        let mut project = Engine::project(&serde_json::Map::new());
        let err = project
            .run(
                r#"magi.hook("bad", { before = function() end })"#,
                ".magi.lua",
            )
            .expect_err("refused");
        assert!(err.to_string().contains("magi.hook"), "{err}");

        let err = project
            .run(
                r#"magi.command("goal", { run = function() end })"#,
                ".magi.lua",
            )
            .expect_err("refused");
        assert!(err.to_string().contains("magi.command"), "{err}");
    }

    #[test]
    fn goal_example_plugin_drives_inline_command_and_turn_loop() {
        let source = include_str!("../../../examples/plugin/goal.lua");
        let mut engine = Engine::new();
        engine.run(source, "goal.lua").expect("load goal plugin");

        let started = crate::commands::dispatch(
            &mut engine,
            &crate::commands::Request {
                prompt: "fix all warnings /goal and run clippy",
                aside: "",
                profile: Some("executor"),
                model: Some("fake/m"),
            },
        );
        assert!(started.matched && !started.handled);
        assert_eq!(started.prompt, "fix all warnings and run clippy");
        assert!(started.aside.contains("GOAL_COMPLETE"));

        let before = before_turn(
            &mut engine,
            &TurnBefore {
                model: "fake/m",
                profile: Some("executor"),
                iteration: 0,
                prompt: Some(&started.prompt),
            },
        );
        assert_eq!(before.system.len(), 1);
        assert!(before.system[0].contains("fix all warnings and run clippy"));

        // Without GOAL_COMPLETE, after_turn requests a continuation.
        let cont = after_turn(
            &mut engine,
            &TurnAfter {
                model: "fake/m",
                profile: Some("executor"),
                iteration: 0,
                text: "Fixed half of them.",
                ok: true,
                cancelled: false,
            },
        );
        assert!(cont.continue_prompt.is_some());

        // With GOAL_COMPLETE, loop finishes cleanly.
        let done = after_turn(
            &mut engine,
            &TurnAfter {
                model: "fake/m",
                profile: Some("executor"),
                iteration: 1,
                text: "All done.\nGOAL_COMPLETE",
                ok: true,
                cancelled: false,
            },
        );
        assert!(done.continue_prompt.is_none());
        assert!(done.notices.iter().any(|n| n.contains("Goal achieved")));

        // After finishing, before_turn stops injecting the goal block.
        let idle = before_turn(
            &mut engine,
            &TurnBefore {
                model: "fake/m",
                profile: Some("executor"),
                iteration: 0,
                prompt: Some("next prompt"),
            },
        );
        assert!(idle.system.is_empty());

        // Cancellation halts an active loop and never continues.
        let _ = crate::commands::dispatch(
            &mut engine,
            &crate::commands::Request {
                prompt: "/goal finish refactor",
                aside: "",
                profile: None,
                model: None,
            },
        );
        let halted = after_turn(
            &mut engine,
            &TurnAfter {
                model: "fake/m",
                profile: None,
                iteration: 0,
                text: "",
                ok: true,
                cancelled: true,
            },
        );
        assert!(halted.continue_prompt.is_none());
        assert!(halted.notices.iter().any(|n| n.contains("halted")));
    }
}
