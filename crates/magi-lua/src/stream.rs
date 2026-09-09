//! The socket primitive the client libraries need.
//!
//! The client carries framing and encoding in plain Lua but cannot open a socket, so the host
//! lends it one as a native rather than a VM feature.
//!
//! ```lua
//! local h = magi.stream.connect(path, timeout_ms)
//! h:send(bytes)   h:recv(n)   h:close()
//! ```

use luna::{Callback, CallbackReturn, Context, Table, Value};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;

/// The most a single `recv` will be asked for, so a peer claiming an enormous frame cannot make
/// us allocate for it before a byte has arrived.
const MAX_RECV: usize = 16 * 1024 * 1024;

type Handle = Rc<RefCell<Option<UnixStream>>>;

/// Whether `path` is a socket a config may dial: this user's own socket directories and nothing
/// else. Narrowed rather than asked, because a config file is read before a session exists and
/// before any `Ops` is lent, which is exactly when an untrusted `.magi.lua` runs. Every legitimate
/// caller puts its sockets under `$XDG_RUNTIME_DIR`. Lexical, on a normalised path, so a name
/// cannot climb out of the directory it appears to be in.
fn dialable(path: &std::path::Path) -> bool {
    roots().iter().any(|root| under(path, root))
}

/// Where this user's sockets may live. Both roots, not one: `magi_ipc::family::socket_dir` falls
/// back to the temporary directory when `$XDG_RUNTIME_DIR` is unset, so a rule naming one root
/// refuses the family's own sockets on half the machines there are.
fn roots() -> Vec<std::path::PathBuf> {
    let mut out = vec![std::env::temp_dir()];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        out.push(std::path::PathBuf::from(runtime));
    }
    out
}

/// Whether `path`, once `..` is resolved, is inside `root`. Split out so a test can pass a root of
/// its own: the alternative is `set_var`, which is `unsafe` and denied here.
fn under(path: &std::path::Path, root: &std::path::Path) -> bool {
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out.starts_with(root)
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
        if !dialable(std::path::Path::new(&path)) {
            stack.replace(
                ctx,
                (
                    Value::Nil,
                    "a socket outside the runtime directory is not this VM's to open",
                ),
            );
            return Ok(CallbackReturn::Return);
        }

        // A stale socket left by a killed peer accepts and never answers.
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
    use super::under;
    use std::path::Path;

    #[test]
    fn only_a_socket_directory_of_this_users_is_dialable() {
        // This callback has no `Ops` in scope, so `ops.allow` is never consulted. Tested against a
        // root of our choosing: reading the real one means `set_var`, which is `unsafe`.
        let root = Path::new("/run/user/1000");

        assert!(under(
            Path::new("/run/user/1000/balthasar/api@1.sock"),
            root
        ));
        assert!(
            under(Path::new("/run/user/1000/oslo/shell.sock"), root),
            "a sibling's own socket"
        );
        assert!(!under(Path::new("/var/run/docker.sock"), root));
        assert!(!under(Path::new("/etc/passwd"), root));
        assert!(
            !under(
                Path::new("/run/user/1000/../../../var/run/docker.sock"),
                root
            ),
            "`..` is resolved before the prefix is compared"
        );
    }
}
