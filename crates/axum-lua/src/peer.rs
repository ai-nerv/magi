//! Calling out to a sibling.
//!
//! axum is a client here, not a server. It loads the sibling's own stub — oslo's `client.lua`,
//! hexe's `hexe.lua` — into its own VM, hands it the socket primitive, and calls a verb. The
//! stub is plain Lua and is copied between tools, so this needs no per-sibling code: a new
//! sibling is a new file, not a new module.
//!
//! This is the direction that makes the family useful to axum. Spawning an agent in a shell is
//! oslo's job and putting it in a tab is hexe's; axum asks them to, rather than reimplementing
//! either.

use crate::{Engine, LuaError};

/// A sibling axum can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sibling {
    /// The tool's name, which is also its global and its socket directory.
    pub name: &'static str,
    /// Where its stub lives, relative to the sibling's checkout.
    pub stub: &'static str,
}

/// The siblings that speak this protocol.
pub const SIBLINGS: &[Sibling] = &[
    Sibling {
        name: "hexe",
        stub: "src/core/lua/hexe.lua",
    },
    Sibling {
        name: "oslo",
        stub: "crates/oslo-runtime/src/lua/api/client.lua",
    },
];

/// The newest control socket a sibling is listening on, if any.
///
/// Discovered here rather than left to the sibling's own stub. A stub looks for the family's
/// globals to find a host that can list a directory, and its list names the siblings that
/// existed when it was written — inside axum, hexe's stub finds neither `_G.hexe` nor
/// `_G.oslo` and falls through to shelling out. Handing it a path sidesteps a question it
/// should not have to answer, and axum knows which session it means anyway.
#[must_use]
pub fn socket_of(name: &str) -> Option<std::path::PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let mut found: Vec<(std::time::SystemTime, std::path::PathBuf)> =
        std::fs::read_dir(runtime.join(name))
            .ok()?
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with("api@") && name.ends_with(".sock")
            })
            .filter_map(|e| {
                let when = e.metadata().ok()?.modified().ok()?;
                Some((when, e.path()))
            })
            .collect();
    // Newest first: a socket left behind by a frontend that was killed looks exactly like a
    // live one until something connects to it.
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().next().map(|(_, path)| path)
}

/// Load a sibling's stub and call one verb on it.
///
/// The stub is given `__stream` explicitly rather than left to find it: a sibling's own
/// discovery looks for its own globals first, and inside axum those are not there.
pub fn call(
    engine: &mut Engine,
    stub: &str,
    verb: &str,
    socket: Option<&str>,
) -> Result<String, LuaError> {
    // A bare string is a session NAME to these stubs, not a path; a path travels in a table.
    // Passing the path as a string found nothing and reported it as "no socket", which is the
    // same message a genuinely absent daemon produces.
    let where_ = socket.map_or_else(|| "nil".to_owned(), |s| format!("{{ path = {s:?} }}"));
    let source = format!(
        r#"
        local chunk = assert(load({stub:?}, "sibling.lua"))
        local sibling = chunk(__stream)
        local peer, why = sibling.connect({where_})
        if not peer then
          axum.answer = "no: " .. tostring(why)
          return
        end
        local ok, result = pcall(function() return peer.{verb}() end)
        peer:close()
        if not ok then
          axum.answer = "refused: " .. tostring(result)
          return
        end
        -- A verb list arrives as records, not names: each entry describes one call.
        -- Reducing them with `tostring` printed table addresses, which looked like an answer.
        if type(result) == "table" then
          local names = {{}}
          for _, v in ipairs(result) do
            names[#names + 1] = type(v) == "table" and (v.name or v[1] or "?") or tostring(v)
          end
          axum.answer = table.concat(names, " ")
        else
          axum.answer = tostring(result)
        end
        "#
    );
    engine.run(&source, "peer.lua")?;
    engine.harvest();
    Ok(engine
        .config()
        .string("answer")
        .unwrap_or("no answer")
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A sibling's stub, if its checkout is beside ours.
    fn stub_of(sibling: Sibling) -> Option<String> {
        let path = PathBuf::from("..").join(sibling.name).join(sibling.stub);
        std::fs::read_to_string(path).ok()
    }

    #[test]
    fn a_sibling_stub_loads_in_axums_vm() {
        // The family's claim is that these files are copied, not ported. It is false the moment
        // one of them cannot run in a sibling's VM, and only a sibling can prove it.
        for sibling in SIBLINGS {
            let Some(stub) = stub_of(*sibling) else {
                continue;
            };
            let mut engine = Engine::new();
            let source = format!(
                r#"
                local chunk = assert(load({stub:?}, "sibling.lua"))
                local m = chunk(__stream)
                axum.answer = m._NAME
                "#
            );
            engine
                .run(&source, "load.lua")
                .unwrap_or_else(|e| panic!("{}'s stub must load in axum: {e}", sibling.name));
            engine.harvest();
            assert_eq!(engine.config().string("answer"), Some(sibling.name));
        }
    }

    #[test]
    fn the_stream_primitive_reports_a_missing_socket_rather_than_hanging() {
        let mut engine = Engine::new();
        engine
            .run(
                r#"
                local h, why = __stream.connect("/nonexistent/axum-test.sock", 200)
                axum.answer = tostring(h) .. "|" .. tostring(why)
                "#,
                "missing.lua",
            )
            .expect("run");
        engine.harvest();
        let answer = engine
            .config()
            .string("answer")
            .expect("an answer")
            .to_owned();
        assert!(answer.starts_with("nil|"), "{answer}");
    }

    #[test]
    fn a_handle_round_trips_bytes() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;

        let path = std::env::temp_dir().join(format!("axum-stream-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut buffer = [0_u8; 5];
            socket.read_exact(&mut buffer).expect("read");
            socket.write_all(b"pong!").expect("write");
        });

        let mut engine = Engine::new();
        engine
            .run(
                &format!(
                    r#"
                    local h = assert(__stream.connect({:?}, 2000))
                    assert(h:send("ping!"))
                    axum.answer = h:recv(5)
                    h:close()
                    "#,
                    path.display().to_string()
                ),
                "roundtrip.lua",
            )
            .expect("run");
        engine.harvest();
        assert_eq!(engine.config().string("answer"), Some("pong!"));

        server.join().expect("server");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_closed_handle_refuses_further_use_rather_than_panicking() {
        let mut engine = Engine::new();
        engine
            .run(
                r#"
                local h = __stream.connect("/nonexistent.sock", 100)
                axum.answer = tostring(h)
                "#,
                "closed.lua",
            )
            .expect("run");
        engine.harvest();
        assert_eq!(engine.config().string("answer"), Some("nil"));
    }
}
