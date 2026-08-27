//! JSON for the VM.
//!
//! A protocol description reads provider payloads, and every one of them is JSON. Lending a
//! parser is cheaper and safer than each description carrying its own — the family's client
//! stubs carry one only because they must run inside hosts that lend nothing.

use crate::convert::{json_from_lua, lua_from_json};
use luna::{Callback, CallbackReturn, Context, Table, Value};

/// Build the `json` table.
pub fn table<'gc>(ctx: Context<'gc>) -> Table<'gc> {
    let json = Table::new(&ctx);

    let decode = Callback::from_fn(&ctx, |ctx, _exec, mut stack| {
        let text: Value = stack.consume(ctx)?;
        let Value::String(text) = text else {
            stack.replace(ctx, Value::Nil);
            return Ok(CallbackReturn::Return);
        };
        // A malformed payload yields nil rather than raising: a stream is a live connection and
        // one unparseable frame must not lose the turn.
        match serde_json::from_slice::<serde_json::Value>(text.as_bytes()) {
            Ok(value) => {
                let value = lua_from_json(ctx, &value);
                stack.replace(ctx, value);
            }
            Err(_) => stack.replace(ctx, Value::Nil),
        }
        Ok(CallbackReturn::Return)
    });
    json.set(ctx, "decode", decode).ok();

    let encode = Callback::from_fn(&ctx, |ctx, _exec, mut stack| {
        let value: Value = stack.consume(ctx)?;
        match json_from_lua(ctx, value, 0).and_then(|v| serde_json::to_string(&v).ok()) {
            Some(text) => {
                let text = luna::String::from_slice(&ctx, text.as_bytes());
                stack.replace(ctx, text);
            }
            None => stack.replace(ctx, Value::Nil),
        }
        Ok(CallbackReturn::Return)
    });
    json.set(ctx, "encode", encode).ok();

    json
}
