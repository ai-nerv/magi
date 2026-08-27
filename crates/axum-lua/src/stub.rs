//! The client library other programs load to talk to a running axum.
//!
//! Plain Lua, and *copied* from the family rather than written: oslo ships `client.lua`, hexe
//! ships `hexe.lua`, and this is the same file with axum's identity and axum's verbs. The
//! framing, the reply shape and the discovery order are shared on purpose, so a fix to any of
//! them reaches every sibling.
//!
//! What it cannot do itself is open a socket. That arrives as the chunk's argument:
//!
//! ```lua
//! load(src)(transport)   -- transport.connect(path, timeout_ms) -> handle
//!                        -- handle:send(bytes) / handle:recv(n) / handle:close()
//! ```

/// The client library, as a program would receive it from `axum lua-api`.
pub const CLIENT: &str = include_str!("../lua/axum.lua");

#[cfg(test)]
mod tests {
    use super::CLIENT;
    use crate::Engine;

    /// Load the stub in axum's own VM and ask it something.
    fn probe(script: &str) -> String {
        let mut engine = Engine::new();
        let source = format!(
            r#"
            local chunk = assert(load({CLIENT:?}, "axum.lua"))
            local axum_client = chunk(nil)
            axum.answer = tostring({script})
            "#
        );
        engine
            .run(&source, "probe.lua")
            .expect("the stub must load");
        engine.harvest();
        engine
            .config()
            .string("answer")
            .expect("an answer")
            .to_owned()
    }

    #[test]
    fn the_stub_loads_in_axums_own_vm() {
        // The family's claim is that the file is copied, not ported. A stub that only ran in
        // the tool that wrote it would make that false the moment a sibling tried it.
        assert_eq!(probe("axum_client._NAME"), "axum");
    }

    #[test]
    fn the_stub_declares_a_protocol_version() {
        assert_eq!(probe("axum_client._VERSION"), "1");
    }

    #[test]
    fn the_stub_offers_connect_and_fetch() {
        // Two verbs, because a lifetime is not an implementation detail: `connect` is a channel
        // you hold, `fetch` is one question with nothing held.
        assert_eq!(probe("type(axum_client.connect)"), "function");
        assert_eq!(probe("type(axum_client.fetch)"), "function");
    }

    #[test]
    fn connecting_without_a_transport_fails_rather_than_hanging() {
        assert_eq!(probe("select(2, axum_client.connect()) ~= nil"), "true");
    }

    #[test]
    fn the_exposed_surface_is_read_only() {
        // A coding agent runs shell commands on the model's say-so, so a verb that hands it a
        // prompt is remote code execution wearing a friendlier name.
        let source = CLIENT;
        let start = source.find("local SURFACE = {").expect("a surface");
        let end = source[start..].find('}').expect("its end") + start;
        let surface = &source[start..end];
        for forbidden in ["prompt", "submit", "run", "exec", "interrupt"] {
            assert!(
                !surface.contains(forbidden),
                "{forbidden:?} causes work and must not be a socket verb"
            );
        }
    }

    #[test]
    fn the_surface_ships_verbs_from_version_one() {
        assert!(
            CLIENT.contains("\"verbs\""),
            "a family where one tool has verbs and another does not stops being one"
        );
    }

    #[test]
    fn the_stub_answers_to_every_sibling_global() {
        // A lookup that knew only its own name would send discovery down the `io.popen` path on
        // exactly the hosts that refuse it, which reads as "nothing is running".
        for sibling in ["axum", "hexe", "oslo"] {
            assert!(
                CLIENT.contains(&format!("\"{sibling}\"")),
                "{sibling} is in the family and must be recognised"
            );
        }
    }

    #[test]
    fn the_frame_is_the_familys_four_byte_big_endian_length() {
        assert!(
            CLIENT.contains("16777216"),
            "the shared framing must not drift"
        );
    }

    #[test]
    fn the_reply_shape_is_a_list_of_return_values() {
        // Two tools disagreeing here fail silently: a client that unpacks reads a bare-value
        // server as having returned nothing at all.
        assert!(CLIENT.contains("reply.result"));
        assert!(CLIENT.contains("reply.n"));
    }
}
