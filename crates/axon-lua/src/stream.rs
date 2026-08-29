//! The socket primitive the client libraries need.
//!
//! Layer one of three: the client carries framing and encoding in plain Lua, but it cannot open a
//! socket, so the host lends it one. A host native like any other — deliberately *not* a VM
//! feature, so a VM that cannot load C modules needs no change to join the family.
//!
//! ```lua
//! local h = axon.stream.connect(path, timeout_ms)
//! h:send(bytes)   h:recv(n)   h:close()
//! ```
//!
//! This is what lets axon dial *out*: oslo's `client.lua` and hexe's `hexe.lua` run unchanged
//! in this VM, given this table.

use luna::{Callback, CallbackReturn, Context, Table, Value};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;

/// The most a single `recv` will be asked for.
///
/// A peer that says a frame is enormous must not make us allocate for it before a byte of it
/// has arrived. The client asks in pieces anyway; this bounds a hostile answer.
const MAX_RECV: usize = 16 * 1024 * 1024;

/// A connected socket, shared between the handle's methods.
type Handle = Rc<RefCell<Option<UnixStream>>>;

/// Build the `stream` table.
pub fn table<'gc>(ctx: Context<'gc>) -> Table<'gc> {
    let stream = Table::new(&ctx);
    let connect = Callback::from_fn(&ctx, |ctx, _exec, mut stack| {
        let (path, timeout_ms): (Value, Value) = stack.consume(ctx)?;
        let Value::String(path) = path else {
            stack.replace(ctx, (Value::Nil, "connect needs a path"));
            return Ok(CallbackReturn::Return);
        };
        let path = String::from_utf8_lossy(path.as_bytes()).into_owned();

        // A default rather than a wait forever: a stale socket left by a killed peer accepts
        // and never answers, which is indistinguishable from a hang without one.
        let timeout = match timeout_ms {
            Value::Integer(ms) if ms > 0 => Duration::from_millis(ms as u64),
            Value::Number(ms) if ms > 0.0 => Duration::from_millis(ms as u64),
            _ => Duration::from_secs(5),
        };

        match UnixStream::connect(&path) {
            Ok(socket) => {
                let _ = socket.set_read_timeout(Some(timeout));
                let _ = socket.set_write_timeout(Some(timeout));
                let handle = handle_table(ctx, Rc::new(RefCell::new(Some(socket))));
                stack.replace(ctx, handle);
            }
            Err(e) => {
                stack.replace(ctx, (Value::Nil, e.to_string()));
            }
        }
        Ok(CallbackReturn::Return)
    });
    stream.set(ctx, "connect", connect).ok();
    stream
}

/// A handle, as the client expects: `send`, `recv`, `close`, called with `:`.
fn handle_table<'gc>(ctx: Context<'gc>, socket: Handle) -> Table<'gc> {
    let handle = Table::new(&ctx);

    let held = Rc::clone(&socket);
    let send = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        // Called as `h:send(bytes)`, so the handle itself is the first argument.
        let (_self, bytes): (Value, Value) = stack.consume(ctx)?;
        let Value::String(bytes) = bytes else {
            stack.replace(ctx, (Value::Nil, "send needs a string"));
            return Ok(CallbackReturn::Return);
        };
        let mut slot = held.borrow_mut();
        let Some(socket) = slot.as_mut() else {
            stack.replace(ctx, (Value::Nil, "the connection is closed"));
            return Ok(CallbackReturn::Return);
        };
        match socket
            .write_all(bytes.as_bytes())
            .and_then(|()| socket.flush())
        {
            Ok(()) => stack.replace(ctx, true),
            Err(e) => stack.replace(ctx, (Value::Nil, e.to_string())),
        }
        Ok(CallbackReturn::Return)
    });
    handle.set(ctx, "send", send).ok();

    let held = Rc::clone(&socket);
    let recv = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        let (_self, want): (Value, Value) = stack.consume(ctx)?;
        let want = match want {
            Value::Integer(n) if n > 0 => (n as usize).min(MAX_RECV),
            Value::Number(n) if n > 0.0 => (n as usize).min(MAX_RECV),
            _ => 0,
        };
        let mut slot = held.borrow_mut();
        let Some(socket) = slot.as_mut() else {
            stack.replace(ctx, (Value::Nil, "the connection is closed"));
            return Ok(CallbackReturn::Return);
        };
        let mut buffer = vec![0_u8; want];
        match socket.read(&mut buffer) {
            // A short read is ordinary, not an error: the client asks again until it has the
            // whole frame. Zero means the peer hung up, and the client reads that as such.
            Ok(read) => {
                buffer.truncate(read);
                let text = luna::String::from_slice(&ctx, &buffer);
                stack.replace(ctx, text);
            }
            Err(e) => stack.replace(ctx, (Value::Nil, e.to_string())),
        }
        Ok(CallbackReturn::Return)
    });
    handle.set(ctx, "recv", recv).ok();

    let held = Rc::clone(&socket);
    let close = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        // Dropping the stream is the close; taking it also makes a second close a no-op rather
        // than an error, which a client's cleanup path relies on.
        held.borrow_mut().take();
        stack.replace(ctx, true);
        Ok(CallbackReturn::Return)
    });
    handle.set(ctx, "close", close).ok();

    handle
}
