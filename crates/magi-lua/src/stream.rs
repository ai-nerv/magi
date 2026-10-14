//! The socket primitive the client libraries need.
//!
//! A client cannot open a socket, so the host lends it one as a native rather than a VM feature,
//! and frames a family call on it in Rust.
//!
//! ```lua
//! local h = magi.stream.connect(path, timeout_ms)
//! local reply, why = h:call({ call = "verbs" }, { deadline_ms = 2000 })
//! h:send(bytes)   h:recv(n)   h:close()
//! ```

mod call;

use luna::{Callback, CallbackReturn, Context, Table, Value};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;

/// The most a single `recv` will be asked for, so a peer claiming an enormous frame cannot make
/// us allocate for it before a byte has arrived.
const MAX_RECV: usize = magi_tools::family::FRAME_CAP;

type Handle = Rc<RefCell<Option<UnixStream>>>;

/// The path to dial when `path` is one of the family's own sockets, and nothing otherwise: its
/// directory, resolved, under `<runtime>/<program>` or `<tmp>/<program>-<uid>` for one of the
/// family's programs, and the entry a socket this uid owns.
fn dialable(path: &std::path::Path) -> Option<std::path::PathBuf> {
    magi_tools::family::family_socket(path).or_else(|| this_sessions_memory(path))
}

/// The memory socket this session was named with, and only that one, held to the same checks.
fn this_sessions_memory(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let named = std::path::Path::new(crate::balthasar_at()?);
    let root = named.parent()?.canonicalize().ok()?;
    let real = magi_tools::family::family_socket_under(path, std::slice::from_ref(&root))?;
    (real == root.join(named.file_name()?)).then_some(real)
}

pub fn table<'gc>(ctx: Context<'gc>) -> Table<'gc> {
    let stream = Table::new(&ctx);
    let connect = Callback::from_fn(&ctx, |ctx, _exec, mut stack| {
        let (path, timeout_ms): (Value, Value) = stack.consume(ctx)?;
        let Value::String(path) = path else {
            stack.replace(ctx, (Value::Nil, "connect needs a path"));
            return Ok(CallbackReturn::Return);
        };
        let path = String::from_utf8_lossy(path.as_bytes()).into_owned();

        // Refused as an ordinary answer, the way a failed connect is: raising would make a config
        // that probed for an absent sibling die instead of carrying on.
        let Some(path) = dialable(std::path::Path::new(&path)) else {
            stack.replace(
                ctx,
                (
                    Value::Nil,
                    "only the family's own sockets are this VM's to open",
                ),
            );
            return Ok(CallbackReturn::Return);
        };

        // A stale socket left by a killed peer accepts and never answers.
        let timeout = match timeout_ms {
            Value::Integer(ms) if ms > 0 => Duration::from_millis(ms as u64),
            Value::Number(ms) if ms > 0.0 => Duration::from_millis(ms as u64),
            _ => Duration::from_secs(5),
        };

        match UnixStream::connect(&path) {
            Ok(socket) if !magi_tools::family::peer_is_us(&socket) => {
                stack.replace(ctx, (Value::Nil, "the socket answered as another user"));
            }
            Ok(socket) => {
                let _ = socket.set_read_timeout(Some(timeout));
                let _ = socket.set_write_timeout(Some(timeout));
                let handle = handle_table(ctx, Rc::new(RefCell::new(Some(socket))), timeout);
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

/// One connection's methods. `timeout` is what `send` and `recv` wait, and what `call` waits when
/// it is given no deadline.
fn handle_table<'gc>(ctx: Context<'gc>, socket: Handle, timeout: Duration) -> Table<'gc> {
    let handle = Table::new(&ctx);

    let held = Rc::clone(&socket);
    let call = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        let (_self, request, opts): (Value, Value, Value) = stack.consume(ctx)?;
        let deadline = std::time::Instant::now() + call::patience(ctx, opts, timeout);
        let frame = match call::framed(ctx, request) {
            Ok(frame) => frame,
            Err(why) => {
                let why = luna::String::from_slice(&ctx, why.as_bytes());
                stack.replace(ctx, (Value::Nil, why));
                return Ok(CallbackReturn::Return);
            }
        };
        let mut slot = held.borrow_mut();
        let Some(socket) = slot.as_mut() else {
            stack.replace(ctx, (Value::Nil, "the connection is closed"));
            return Ok(CallbackReturn::Return);
        };
        let answered = call::exchange(socket, &frame, deadline);
        let restored = socket
            .set_read_timeout(Some(timeout))
            .and_then(|()| socket.set_write_timeout(Some(timeout)))
            .map_err(|why| why.to_string());
        // A call that did not come back whole closes the handle.
        match answered.and_then(|reply| restored.map(|()| reply)) {
            Ok(reply) => stack.replace(ctx, crate::convert::lua_from_json(ctx, &reply)),
            Err(why) => {
                slot.take();
                let why = luna::String::from_slice(&ctx, why.as_bytes());
                stack.replace(ctx, (Value::Nil, why));
            }
        }
        Ok(CallbackReturn::Return)
    });
    handle.set(ctx, "call", call).ok();

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
            // A short read is ordinary: the client asks again until it has the whole frame. Zero
            // means the peer hung up.
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
        // Taking the stream makes a second close a no-op, which a client's cleanup relies on.
        held.borrow_mut().take();
        stack.replace(ctx, true);
        Ok(CallbackReturn::Return)
    });
    handle.set(ctx, "close", close).ok();

    handle
}

#[cfg(test)]
mod tests {
    use super::dialable;
    use std::path::Path;

    #[test]
    fn nothing_beside_the_family_is_dialable() {
        // The session bus and every other listener sharing the runtime directory are refused,
        // whether or not anything listens there.
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        for path in [
            runtime.join("bus"),
            runtime.join("systemd/private"),
            runtime.join("magi/../bus"),
            Path::new("/var/run/docker.sock").to_path_buf(),
            Path::new("/etc/passwd").to_path_buf(),
        ] {
            assert!(dialable(&path).is_none(), "{}", path.display());
        }
    }
}
