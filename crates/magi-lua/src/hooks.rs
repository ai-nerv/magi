//! Pre/post tool hooks registered in Lua via `magi.hook(name, spec)`.

use crate::Engine;
use crate::convert::{json_from_lua, lua_from_json};
use magi_tools::hooking::{After, Before, Hook};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) const HOOKS: &str = "__magi_hooks";

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
    }
}
