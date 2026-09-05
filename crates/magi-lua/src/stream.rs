//! The socket primitive the client libraries need.
//!
//! Layer one of three: the client carries framing and encoding in plain Lua, but it cannot open a
//! socket, so the host lends it one. A host native like any other — deliberately *not* a VM
//! feature, so a VM that cannot load C modules needs no change to join the family.
//!
//! ```lua
//! local h = magi.stream.connect(path, timeout_ms)
//! h:send(bytes)   h:recv(n)   h:close()
//! ```
//!
//! This is what lets magi dial *out*: oslo's `client.lua` and hexe's `hexe.lua` run unchanged
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

/// Whether `path` is a socket a config may dial.
///
/// **This user's own socket directories and nothing else.** This primitive used to call `UnixStream::connect`
/// on whatever it was handed, from a callback with no [`magi_tools::Ops`] in scope at all — so
/// `ops.allow` was never consulted, `Action::Network` was never constructed for it, and any Lua
/// a config could reach could open any socket this user can: the host's own control socket, a
/// sibling's, a container runtime's.
///
/// Narrowed rather than asked, because there is no one place to ask from. A config file is read
/// before a session exists and before any `Ops` is lent, and that read is exactly when an
/// untrusted `.magi.lua` runs — so a check that only worked inside a session would be absent in
/// the window that matters most. Every legitimate caller is already here: oslo, hexe and
/// balthasar all put their sockets under `$XDG_RUNTIME_DIR`, and so does magi.
///
/// Lexical, on a normalised path: `..` is resolved first, so a name cannot climb out of the
/// directory it appears to be in.
fn dialable(path: &std::path::Path) -> bool {
    roots().iter().any(|root| under(path, root))
}

/// Where this user's sockets may live.
///
/// Both, not one. `magi_ipc::family::socket_dir` uses `$XDG_RUNTIME_DIR` when it is set and falls
/// back to the temporary directory when it is not, so a rule naming only the first refuses the
/// family's own sockets on any machine without that variable — and a rule naming only the second
/// refuses them everywhere else. Checking one root broke `peer`'s round-trip test the moment it
/// landed, which is exactly the case a real deployment without `$XDG_RUNTIME_DIR` would hit.
fn roots() -> Vec<std::path::PathBuf> {
    let mut out = vec![std::env::temp_dir()];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        out.push(std::path::PathBuf::from(runtime));
    }
    out
}

/// Whether `path`, once `..` is resolved, is inside `root`.
///
/// Split out so it can be tested against a root of the test's choosing: the alternative is
/// setting `XDG_RUNTIME_DIR`, and `std::env::set_var` is `unsafe`, which this workspace denies.
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

        // Refused as an ordinary answer, the way a failed connect already is: `nil` and a reason
        // the caller can put on screen. Raising would make a config that probed for an absent
        // sibling die instead of carrying on without it.
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

#[cfg(test)]
mod tests {
    use super::under;
    use std::path::Path;

    #[test]
    fn only_a_socket_directory_of_this_users_is_dialable() {
        // The hole this closes: this callback has no `Ops` in scope, so `ops.allow` was never
        // consulted and `Action::Network` was never built for it. Any Lua a config could reach
        // could open any socket this user can -- the host's own control socket included.
        //
        // Tested against a root of our choosing rather than the real one: reading the answer out
        // of the environment would mean setting a variable, and `set_var` is `unsafe`.
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
