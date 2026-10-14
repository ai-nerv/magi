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

    /// A transport whose connection hands back `replies`, one `recv` at a time, and counts them.
    const SCRIPTED: &str = r#"
        local replies, recvs = __replies, 0
        __transport = { connect = function() return {
          send = function(_, bytes) return true end,
          recv = function(_, n) recvs = recvs + 1; __recvs = recvs; return table.remove(replies, 1) or "" end,
          close = function() end,
        } end }
    "#;

    fn framed(body: &str) -> String {
        let n = body.len();
        let header: String = [n >> 24, n >> 16, n >> 8, n]
            .iter()
            .map(|b| format!("\\{}", b & 0xff))
            .collect();
        format!("\"{header}\", {body:?}")
    }

    #[test]
    fn a_declared_length_past_the_cap_is_refused_before_the_body_is_read() {
        let answer = probe(&format!(
            r#"(function()
                __replies = {{ "\255\255\255\255" }}
                {SCRIPTED}
                local s = assert(chunk(__transport).connect({{ path = "/x" }}))
                local v, why = s:call("verbs")
                return tostring(v) .. "|" .. tostring(why) .. "|" .. tostring(__recvs)
            end)()"#
        ));
        assert!(answer.starts_with("nil|the reply declared"), "{answer}");
        assert!(answer.ends_with("|1"), "the body was asked for: {answer}");
    }

    #[test]
    fn a_reply_in_a_newer_family_is_refused_by_name() {
        let answer = probe(&format!(
            r#"(function()
                __replies = {{ {} }}
                {SCRIPTED}
                local s = assert(chunk(__transport).connect({{ path = "/x" }}))
                local v, why = s:call("verbs")
                return tostring(v) .. "|" .. tostring(why)
            end)()"#,
            framed(r#"{"ok":true,"family":2,"n":1,"result":[1]}"#)
        ));
        assert!(
            answer.starts_with("nil|") && answer.contains("family 2"),
            "{answer}"
        );
    }

    #[test]
    fn a_descriptor_somebody_else_could_write_is_never_executed() {
        // The host says who this user is and what the file is; `$UID` is never asked.
        let run = |mode: u32, uid: &str| {
            probe(&format!(
                r#"(function()
                    magi.fs = nil
                    local ran = "nothing"
                    oslo = {{ uid = {uid}, run = function(cmd) ran = cmd
                        return {{ ok = true, stdout = '{{"ok":true,"n":1,"result":["x"]}}' }} end,
                      fs = {{ ls = function() return {{}} end,
                              read = function() return '{{"exec":"/opt/x/tool"}}' end,
                              stat = function() return {{ uid = 1000, mode = {mode} }} end }} }}
                    chunk(nil).fetch({{ tool = "x" }}, "verbs")
                    oslo = nil
                    return ran
                end)()"#
            ))
        };
        assert_ne!(
            run(0o644, "1000"),
            "nothing",
            "its own, unwritable by others, runs"
        );
        assert_eq!(run(0o664, "1000"), "nothing", "group-writable");
        assert_eq!(run(0o646, "1000"), "nothing", "other-writable");
        assert_eq!(run(0o644, "1001"), "nothing", "somebody else's");
        assert_eq!(run(0o644, "nil"), "nothing", "no uid from the host");
    }

    #[test]
    fn the_reply_shape_is_a_list_of_return_values() {
        // A client that unpacks reads a bare-value server as having returned nothing at all.
        assert!(CLIENT.contains("reply.result"));
        assert!(CLIENT.contains("reply.n"));
    }
}
