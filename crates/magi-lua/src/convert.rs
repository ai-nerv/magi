//! Turning what the config declared into owned Rust values.
//!
//! Conversion happens once, at the boundary, because luna's `Value<'gc>` cannot leave
//! `lua.enter`. Everything above this crate sees `serde_json::Value`, which every other crate
//! already speaks — a provider declaration reaches `magi-provider` in the same shape whether it
//! came from Lua or from a file.

use luna::{Table, Value};
use serde_json::{Map, Number};

/// A Lua value as JSON.
///
/// Lua does not distinguish a list from a map, so a table whose keys are exactly `1..n` becomes
/// an array and anything else an object. That is the same rule the family's client library uses,
/// and disagreeing with it would make a config and a socket describe one table two ways.
#[must_use]
pub fn json_from_lua<'gc>(
    ctx: luna::Context<'gc>,
    value: Value<'gc>,
    depth: usize,
) -> Option<serde_json::Value> {
    // Bounded because a config is user input and a cyclic table is a stack overflow, not an
    // error, if this recurses freely.
    if depth > 32 {
        return None;
    }
    Some(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(b),
        Value::Integer(i) => serde_json::Value::Number(i.into()),
        Value::Number(f) => Number::from_f64(f).map_or(serde_json::Value::Null, Into::into),
        Value::String(s) => serde_json::Value::String(String::from_utf8_lossy(s.as_bytes()).into()),
        Value::Table(t) => table_to_json(ctx, t, depth)?,
        // A function cannot cross the boundary: what is stored is a declaration, and a callback
        // belongs to the VM that made it. Registrars keep those separately.
        _ => return None,
    })
}

fn table_to_json<'gc>(
    ctx: luna::Context<'gc>,
    table: Table<'gc>,
    depth: usize,
) -> Option<serde_json::Value> {
    let entries: Vec<(Value<'gc>, Value<'gc>)> = table.iter(ctx).collect();

    let is_list = !entries.is_empty()
        && entries
            .iter()
            .enumerate()
            .all(|(index, (key, _))| matches!(key, Value::Integer(i) if *i == index as i64 + 1));

    if is_list {
        let mut out = Vec::with_capacity(entries.len());
        for (_, value) in entries {
            out.push(json_from_lua(ctx, value, depth + 1)?);
        }
        return Some(serde_json::Value::Array(out));
    }

    let mut out = Map::new();
    for (key, value) in entries {
        let name = match key {
            Value::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            Value::Integer(i) => i.to_string(),
            // A key that is not a name cannot be written down; skipping is wrong, so the whole
            // table is refused and the caller reports which declaration was malformed.
            _ => return None,
        };
        out.insert(name, json_from_lua(ctx, value, depth + 1)?);
    }
    Some(serde_json::Value::Object(out))
}

/// Read a declaration out of a Lua table.
pub trait FromLua: Sized {
    /// Build from the JSON a config table converted to.
    ///
    /// # Errors
    /// When the table does not match the type's shape.
    fn from_json(value: serde_json::Value) -> Result<Self, String>;
}

impl<T: serde::de::DeserializeOwned> FromLua for T {
    fn from_json(value: serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

/// A JSON value as Lua.
///
/// The inverse of [`json_from_lua`], and the direction an adapter needs: a model, a context and
/// a set of options are Rust values that a Lua function has to be handed.
///
/// An empty array and an empty object both become an empty table, because Lua has one table
/// type. Adapters must not distinguish them — and none of the ten protocols does.
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
                // Lua arrays are 1-based, and an adapter written against 0 would silently read
                // one element short of the conversation.
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

/// A Lua table as JSON, dropping anything that cannot be described.
///
/// [`json_from_lua`] refuses a whole table containing a function, which is right for a value
/// crossing the boundary: silently losing a field would be worse than refusing. A *declaration*
/// is the other case — a tool spec deliberately holds its `run` function beside describable
/// fields, and refusing the table would drop the tool.
///
/// So the two are separate functions rather than a flag: the strict one is the default, and
/// choosing to lose something is written down at the call site.
#[must_use]
pub fn declaration_from_lua<'gc>(
    ctx: luna::Context<'gc>,
    value: Value<'gc>,
    depth: usize,
) -> Option<serde_json::Value> {
    if depth > 32 {
        return None;
    }
    let Value::Table(table) = value else {
        return json_from_lua(ctx, value, depth);
    };

    let entries: Vec<(Value<'gc>, Value<'gc>)> = table.iter(ctx).collect();
    let is_list = !entries.is_empty()
        && entries
            .iter()
            .enumerate()
            .all(|(index, (key, _))| matches!(key, Value::Integer(i) if *i == index as i64 + 1));

    if is_list {
        let items: Vec<serde_json::Value> = entries
            .into_iter()
            .filter_map(|(_, value)| declaration_from_lua(ctx, value, depth + 1))
            .collect();
        return Some(serde_json::Value::Array(items));
    }

    let mut out = serde_json::Map::new();
    for (key, value) in entries {
        let name = match key {
            Value::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            Value::Integer(i) => i.to_string(),
            _ => continue,
        };
        if let Some(json) = declaration_from_lua(ctx, value, depth + 1) {
            out.insert(name, json);
        }
    }
    Some(serde_json::Value::Object(out))
}
