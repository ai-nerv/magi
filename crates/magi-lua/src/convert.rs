//! Turning what the config declared into owned Rust values. Conversion happens once, at the
//! boundary, because luna's `Value<'gc>` cannot leave `lua.enter`; everything above this crate
//! sees `serde_json::Value`.

use luna::{Table, Value};
use serde_json::{Map, Number};
use std::collections::HashSet;

/// The deepest one conversion may nest tables.
pub const DEEPEST: usize = 32;

/// The most values one conversion may write, keys included.
pub const NODES: usize = 1 << 21;

/// The most string bytes one conversion may write, keys included.
pub const BYTES: usize = 4 * magi_tools::family::FRAME_CAP;

/// A Lua value as JSON. A table whose keys are exactly `1..n` becomes an array and anything else
/// an object, which is the rule the family's client library uses.
///
/// # Errors
/// When the value holds a function or a key that is not a name, holds itself, nests deeper than
/// [`DEEPEST`], or comes to more than [`NODES`] values or [`BYTES`] of text.
pub fn json_from_lua<'gc>(
    ctx: luna::Context<'gc>,
    value: Value<'gc>,
) -> Result<serde_json::Value, String> {
    Ok(Walk::new(ctx, false)
        .value(value)?
        .unwrap_or(serde_json::Value::Null))
}

/// A Lua table as JSON, dropping anything that cannot be described rather than refusing the whole:
/// a tool spec holds its `run` function beside describable fields. `None` when nothing is left.
///
/// # Errors
/// When the value holds itself, nests deeper than [`DEEPEST`], or comes to more than [`NODES`]
/// values or [`BYTES`] of text.
pub fn declaration_from_lua<'gc>(
    ctx: luna::Context<'gc>,
    value: Value<'gc>,
) -> Result<Option<serde_json::Value>, String> {
    Walk::new(ctx, true).value(value)
}

/// One conversion's running totals, and the tables it is inside.
struct Walk<'gc> {
    ctx: luna::Context<'gc>,
    nodes: usize,
    bytes: usize,
    open: HashSet<Table<'gc>>,
    /// Drop what cannot be described, rather than refuse the whole value.
    dropping: bool,
}

impl<'gc> Walk<'gc> {
    fn new(ctx: luna::Context<'gc>, dropping: bool) -> Self {
        Self {
            ctx,
            nodes: 0,
            bytes: 0,
            open: HashSet::new(),
            dropping,
        }
    }

    /// One value, or `None` for one that is dropped.
    fn value(&mut self, value: Value<'gc>) -> Result<Option<serde_json::Value>, String> {
        self.spend()?;
        Ok(Some(match value {
            Value::Nil => serde_json::Value::Null,
            Value::Boolean(b) => serde_json::Value::Bool(b),
            Value::Integer(i) => serde_json::Value::Number(i.into()),
            Value::Number(f) => Number::from_f64(f).map_or(serde_json::Value::Null, Into::into),
            Value::String(s) => serde_json::Value::String(self.text(s)?),
            Value::Table(t) => return self.table(t),
            _ if self.dropping => return Ok(None),
            other => return Err(format!("a {} cannot be described", other.type_name())),
        }))
    }

    fn spend(&mut self) -> Result<(), String> {
        self.nodes += 1;
        if self.nodes > NODES {
            return Err(format!("more than {NODES} values"));
        }
        Ok(())
    }

    fn text(&mut self, text: luna::String<'gc>) -> Result<String, String> {
        self.bytes += text.as_bytes().len();
        if self.bytes > BYTES {
            return Err(format!("more than {} MiB of text", BYTES >> 20));
        }
        Ok(String::from_utf8_lossy(text.as_bytes()).into_owned())
    }

    fn table(&mut self, table: Table<'gc>) -> Result<Option<serde_json::Value>, String> {
        if self.open.len() >= DEEPEST {
            return Err(format!("nested deeper than {DEEPEST} tables"));
        }
        if !self.open.insert(table) {
            return Err("a table holds itself".to_owned());
        }
        // One past what the budget has left, so an over-wide table is refused unread.
        let room = NODES.saturating_sub(self.nodes) + 1;
        let entries: Vec<(Value<'gc>, Value<'gc>)> = table.iter(self.ctx).take(room).collect();
        let out = self.entries(entries);
        self.open.remove(&table);
        out.map(Some)
    }

    fn entries(
        &mut self,
        entries: Vec<(Value<'gc>, Value<'gc>)>,
    ) -> Result<serde_json::Value, String> {
        let is_list = !entries.is_empty()
            && entries.iter().enumerate().all(
                |(index, (key, _))| matches!(key, Value::Integer(i) if *i == index as i64 + 1),
            );

        if is_list {
            let mut out = Vec::with_capacity(entries.len());
            for (_, value) in entries {
                out.extend(self.value(value)?);
            }
            return Ok(serde_json::Value::Array(out));
        }

        let mut out = Map::new();
        for (key, value) in entries {
            self.spend()?;
            let name = match key {
                Value::String(s) => self.text(s)?,
                Value::Integer(i) => i.to_string(),
                _ if self.dropping => continue,
                other => {
                    return Err(format!(
                        "a key of type {} cannot be described",
                        other.type_name()
                    ));
                }
            };
            if let Some(json) = self.value(value)? {
                out.insert(name, json);
            }
        }
        Ok(serde_json::Value::Object(out))
    }
}

pub trait FromLua: Sized {
    /// Build from the JSON a config table converted to. Errors when the table does not fit.
    fn from_json(value: serde_json::Value) -> Result<Self, String>;
}

impl<T: serde::de::DeserializeOwned> FromLua for T {
    fn from_json(value: serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

/// A JSON value as Lua, the direction an adapter needs. An empty array and an empty object both
/// become an empty table, because Lua has one table type, and adapters must not distinguish them.
pub fn lua_from_json<'gc>(ctx: luna::Context<'gc>, value: &serde_json::Value) -> Value<'gc> {
    match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::Number(n.as_f64().unwrap_or(0.0)), Value::Integer),
        serde_json::Value::String(s) => Value::String(luna::String::from_slice(&ctx, s.as_bytes())),
        serde_json::Value::Array(items) => {
            let table = Table::new(&ctx);
            for (index, item) in items.iter().enumerate() {
                // Lua arrays are 1-based, and an adapter written against 0 would read one element
                // short of the conversation.
                table
                    .set(ctx, index as i64 + 1, lua_from_json(ctx, item))
                    .ok();
            }
            Value::Table(table)
        }
        serde_json::Value::Object(fields) => {
            let table = Table::new(&ctx);
            for (key, item) in fields {
                let key = luna::String::from_slice(&ctx, key.as_bytes());
                table.set(ctx, key, lua_from_json(ctx, item)).ok();
            }
            Value::Table(table)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BYTES, DEEPEST, NODES};
    use crate::Engine;

    /// What a chunk left in `magi.said`, run in a fresh engine.
    fn said(source: &str) -> String {
        let mut engine = Engine::new();
        engine.run(source, "convert.lua").expect("runs");
        engine.harvest();
        engine
            .config()
            .string("said")
            .unwrap_or("<absent>")
            .to_owned()
    }

    /// What `json.encode` answered for the table `t` that `build` made, as `text|why`.
    fn encoded(build: &str) -> String {
        said(&format!(
            "{build}\nlocal text, why = magi.json.encode(t)\n\
             magi.said = tostring(text) .. '|' .. tostring(why)"
        ))
    }

    #[test]
    fn a_value_that_holds_itself_is_refused_by_name_wherever_it_leaves() {
        let said = encoded("local t = { n = 1 } t.again = { t }");
        assert_eq!(said, "nil|a table holds itself");
        let mut engine = Engine::new();
        engine
            .run(
                "magi.loop = {} magi.loop.again = magi.loop\n\
                 magi.tool('loop', { run = function() local t = {} t[1] = t return t end })",
                "loop.lua",
            )
            .expect("runs");
        engine.harvest();
        let unkept = engine.config().unkept.join("\n");
        assert!(
            unkept.contains("magi.loop was not kept: a table holds itself"),
            "{unkept}"
        );
        let answer = engine
            .call_tool("loop", &serde_json::Value::Null)
            .expect("an answer");
        assert_eq!(answer["is_error"], true, "{answer}");
        assert!(
            answer["content"]
                .as_str()
                .is_some_and(|c| c.contains("holds itself"))
        );
    }

    #[test]
    fn a_table_shared_twice_is_written_twice_and_is_not_a_cycle() {
        let said = encoded("local s = { 1, 2 } local t = { a = s, b = { s } }");
        let text = said.strip_suffix("|nil").expect(&said);
        let back: serde_json::Value = serde_json::from_str(text).expect(text);
        assert_eq!(back, serde_json::json!({ "a": [1, 2], "b": [[1, 2]] }));
    }

    #[test]
    fn sharing_that_multiplies_past_the_node_budget_is_refused() {
        let said = encoded("local t = 0 for _ = 1, 20 do t = { t, t, t, t } end");
        assert_eq!(said, format!("nil|more than {NODES} values"));
    }

    #[test]
    fn strings_past_the_byte_budget_are_refused() {
        let said =
            encoded("local s = ('x'):rep(1 << 20) local t = {} for i = 1, 65 do t[i] = s end");
        assert_eq!(said, format!("nil|more than {} MiB of text", BYTES >> 20));
    }

    #[test]
    fn nesting_up_to_the_deepest_is_written_and_past_it_refused() {
        let nest = |depth: usize| format!("local t = 1 for _ = 1, {depth} do t = {{ t }} end");
        assert!(encoded(&nest(DEEPEST)).ends_with("|nil"));
        assert_eq!(
            encoded(&nest(DEEPEST + 1)),
            format!("nil|nested deeper than {DEEPEST} tables")
        );
    }

    #[test]
    fn a_declaration_drops_its_functions_and_keeps_the_rest() {
        let mut engine = Engine::new();
        engine
            .run(
                "magi.tool('t', { description = 'd', run = function() end, [true] = 1 })",
                "declare.lua",
            )
            .expect("runs");
        assert_eq!(
            engine.tools(),
            [("t".to_owned(), serde_json::json!({ "description": "d" }))]
        );
    }
}
