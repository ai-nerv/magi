//! The client library other programs load to talk to a running magi.
//!
//! Copied from the family rather than written, so a fix to the reply shape or the discovery order
//! reaches every sibling. Opening a socket and framing a call arrive as the chunk's argument:
//!
//! ```lua
//! load(src)(transport)   -- transport.connect(path, timeout_ms) -> handle
//!                        -- handle:call(request, { deadline_ms = N }) -> reply | nil, why
//!                        -- handle:close()
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
    fn the_client_frames_nothing_by_hand() {
        assert!(CLIENT.contains("handle:call(body, { deadline_ms = self.timeout_ms })"));
        for framing in [":recv(", ":send(", "be32", "16777216"] {
            assert!(!CLIENT.contains(framing), "{framing:?} is framing by hand");
        }
    }

    /// A transport whose connection answers each `call` with the next of `__replies`, and keeps
    /// what it was asked.
    const SCRIPTED: &str = r#"
        __transport = { connect = function() return {
          call = function(_, request, opts)
            __asked, __deadline = request, opts.deadline_ms
            return table.remove(__replies, 1)
          end,
          close = function() end,
        } end }
    "#;

    #[test]
    fn a_call_is_one_request_table_on_the_sessions_deadline() {
        let answer = probe(&format!(
            r#"(function()
                __replies = {{ {{ ok = true, n = 2, result = {{ "a", "b" }} }} }}
                {SCRIPTED}
                local s = assert(chunk(__transport).connect({{ path = "/x", timeout_ms = 900 }}))
                local first, second = s:call("session", "main")
                return first .. second .. "|" .. __asked.call .. "|" .. __asked.args[1] .. "|" .. __deadline
            end)()"#
        ));
        assert_eq!(answer, "ab|session|main|900");
    }

    #[test]
    fn a_transport_that_lends_no_call_is_refused_by_name() {
        let answer = probe(
            r#"(function()
                local bare = { connect = function() return { close = function() end } end }
                local s = assert(chunk(bare).connect({ path = "/x" }))
                local v, why = s:call("verbs")
                return tostring(v) .. "|" .. tostring(why)
            end)()"#,
        );
        assert!(
            answer.starts_with("nil|") && answer.contains("handle:call"),
            "{answer}"
        );
    }

    #[test]
    fn a_reply_in_a_newer_family_is_refused_by_name() {
        let answer = probe(&format!(
            r#"(function()
                __replies = {{ {{ ok = true, family = 2, n = 1, result = {{ 1 }} }} }}
                {SCRIPTED}
                local s = assert(chunk(__transport).connect({{ path = "/x" }}))
                local v, why = s:call("verbs")
                return tostring(v) .. "|" .. tostring(why)
            end)()"#
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
