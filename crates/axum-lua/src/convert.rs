//! Turning what the config declared into owned Rust values.
//!
//! Conversion happens once, at the boundary, because luna's `Value<'gc>` cannot leave
//! `lua.enter`. Everything above this crate sees `serde_json::Value`, which every other crate
//! already speaks — a provider declaration reaches `axum-provider` in the same shape whether it
//! came from Lua or from a file.

use luna::{Table, Value};
use serde_json::{Map, Number};

/// A Lua value as JSON.
///
/// Lua does not distinguish a list from a map, so a table whose keys are exactly `1..n` becomes
/// an array and anything else an object. That is the same rule the family's client stub uses,
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
