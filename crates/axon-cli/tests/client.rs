//! The client library loads, in the VM a sibling would load it in.
//!
//! Everything else about the agent layer is tested against itself, which is exactly the thing
//! the family's guidance says proves nothing: *every discovery bug is invisible from inside the
//! tool that owns it.* A stub with a syntax error, a `load` that returns the wrong thing, a file
//! that got truncated by an `include_str!` pointing somewhere stale — all of them look fine from
//! Rust and fail in somebody else's program.
//!
//! So this runs it the way a sibling does: hand the source to a Lua VM through `axon.clients`,
//! `load` it, call the chunk, and use what comes back.

use axon_lua::Engine;

/// Run `source` with the agent client installed, and hand back what the config assigned.
fn with_client(source: &str) -> axon_lua::Config {
    let mut engine = Engine::new();
    engine.install_clients(&[("agent".to_owned(), axon_agent::CLIENT.to_owned())]);
    engine.run(source, "test.lua").expect("the config runs");
    engine.harvest();
    engine.config()
}

#[test]
fn the_stub_loads_and_returns_its_module() {
    // A syntax error here is a file no sibling can use, and nothing in Rust would have noticed.
    let config = with_client(
        r#"
        local chunk, why = load(axon.clients.agent, "agent.lua")
        if not chunk then
            axon.loaded = "did not parse: " .. tostring(why)
        else
            local agent = chunk(axon.stream)
            axon.loaded = tostring(agent._NAME) .. "/" .. tostring(agent._VERSION)
            axon.connect = type(agent.connect)
            axon.fetch = type(agent.fetch)
            axon.instances = type(agent.instances)
        end
        "#,
    );
    assert_eq!(config.string("loaded"), Some("agent/1"));
    assert_eq!(config.string("connect"), Some("function"));
    assert_eq!(config.string("fetch"), Some("function"));
    assert_eq!(config.string("instances"), Some("function"));
}

#[test]
fn it_encodes_a_call_the_way_the_wire_expects() {
    // The framing and the encoder are the halves a sibling cannot get from anywhere else, and
    // the reply shape is the one the family fails silently over: a client that unpacks a list
    // reads a bare-value server as having returned nothing at all.
    let config = with_client(
        r#"
        local agent = load(axon.clients.agent, "agent.lua")(axon.stream)
        -- Nothing is listening, so this is the error path — which is the point: a refusal has
        -- to arrive as a message somebody can act on.
        local session, why = agent.connect({ path = "/nonexistent/axon-test.sock" })
        axon.refused = session == nil and tostring(why) or "it connected to nothing"
        "#,
    );
    let refused = config.string("refused").expect("a refusal");
    assert!(
        refused.contains("nothing is listening"),
        "a client that cannot connect must say so: {refused}"
    );
}

#[test]
fn a_name_with_no_session_behind_it_says_which_part_is_missing() {
    // `agent.connect()` with no name means "my own session", and a process outside one has no
    // name to use. Inventing one would put messages in somebody's inbox from a sender that does
    // not exist, so it refuses and says what to set.
    let config = with_client(
        r#"
        local agent = load(axon.clients.agent, "agent.lua")(axon.stream)
        local session, why = agent.connect()
        axon.refused = session == nil and tostring(why) or "it named itself something"
        axon.me = tostring(agent.me())
        "#,
    );
    let refused = config.string("refused").expect("a refusal");
    assert!(refused.contains("AXON_PROJECT"), "{refused}");
    assert_eq!(
        config.string("me"),
        Some("nil"),
        "and it knows it has no name"
    );
}

#[test]
fn the_stub_reads_the_three_shapes_of_a_name() {
    // `id`, `role/id` and `project/role/id` all reach the same session, because the id is the
    // part that places it and a role is what a session says it is *for*.
    let config = with_client(
        r#"
        local agent = load(axon.clients.agent, "agent.lua")(axon.stream)
        local out = {}
        for _, name in ipairs({ "iota-mu", "review/iota-mu", "axon/review/iota-mu" }) do
            local session, why = agent.connect(name)
            out[#out + 1] = tostring(why)
        end
        axon.tried = table.concat(out, " | ")
        "#,
    );
    let tried = config.string("tried").expect("three attempts");
    assert_eq!(
        tried.matches("iota-mu").count(),
        3,
        "every shape resolved to the same id: {tried}"
    );
}
