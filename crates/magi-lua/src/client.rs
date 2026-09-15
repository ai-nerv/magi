//! The client library other programs load to talk to a running magi.
//!
//! Copied from the family rather than written, so a fix to the framing, the reply shape or the
//! discovery order reaches every sibling. Opening a socket arrives as the chunk's argument:
//!
//! ```lua
//! load(src)(transport)   -- transport.connect(path, timeout_ms) -> handle
//!                        -- handle:send(bytes) / handle:recv(n) / handle:close()
//! ```

/// The client library, as a program would receive it from `magi lua-api`.
pub const CLIENT: &str = include_str!("../lua/magi.lua");

#[cfg(test)]
mod tests {
    use super::CLIENT;
    use crate::Engine;

    fn probe(script: &str) -> String {
        let mut engine = Engine::new();
        let source = format!(
            r#"
            local chunk = assert(load({CLIENT:?}, "magi.lua"))
            local magi_client = chunk(nil)
            magi.answer = tostring({script})
            "#
        );
        engine
            .run(&source, "probe.lua")
            .expect("the client must load");
        engine.harvest();
        engine
            .config()
            .string("answer")
            .expect("an answer")
            .to_owned()
    }

    #[test]
    fn the_stub_loads_in_magis_own_vm() {
        // The family's claim is that the file is copied, not ported.
        assert_eq!(probe("magi_client._NAME"), "magi");
    }

    #[test]
    fn the_stub_declares_a_protocol_version() {
        assert_eq!(probe("magi_client._VERSION"), "1");
    }

    #[test]
    fn the_stub_offers_connect_and_fetch() {
        // Two verbs: `connect` is a channel you hold, `fetch` is one question with nothing held.
        assert_eq!(probe("type(magi_client.connect)"), "function");
        assert_eq!(probe("type(magi_client.fetch)"), "function");
    }

    #[test]
    fn connecting_without_a_transport_fails_rather_than_hanging() {
        assert_eq!(probe("select(2, magi_client.connect()) ~= nil"), "true");
    }

    #[test]
    fn the_exposed_surface_is_read_only() {
        // A verb that hands a coding agent a prompt is remote code execution under another name.
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
        // exactly the hosts that refuse it.
        for sibling in ["magi", "hexe", "oslo"] {
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
        // A client that unpacks reads a bare-value server as having returned nothing at all.
        assert!(CLIENT.contains("reply.result"));
        assert!(CLIENT.contains("reply.n"));
    }
}
