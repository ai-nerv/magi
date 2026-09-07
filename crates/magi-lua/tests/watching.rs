//! Events reaching the place watchers are actually written.
//!
//! The Rust side of this is checked in `magi-tools`: an event has a name, a registry passes it
//! on. What that cannot show is whether any of it arrives in Lua, which is where every watcher
//! anybody writes will live — and the seam between them is a JSON value and a string, neither of
//! which the compiler checks.

use magi_lua::Engine;
use magi_tools::{Registry, Watch};
use std::cell::RefCell;
use std::rc::Rc;

/// An engine with a watcher that appends the kind of everything it sees to a global.
fn watching() -> Rc<RefCell<Engine>> {
    let mut engine = Engine::new();
    engine
        .run(
            r#"
            __seen = {}
            magi.watch("everything", {
              run = function(event)
                __seen[#__seen + 1] = event.kind .. ":" .. tostring(event.model or event.dropped or event.verb or event.id or event.tool)
              end,
            })
            "#,
            "test",
        )
        .expect("config");
    engine.harvest();
    Rc::new(RefCell::new(engine))
}

/// What the watcher wrote down.
///
/// Read back out through a setting, which is the only channel the engine exposes to a caller —
/// and the same one a real configuration would use to act on what it saw.
fn seen(engine: &Rc<RefCell<Engine>>) -> Vec<String> {
    let mut engine = engine.borrow_mut();
    engine
        .run("magi.seen = table.concat(__seen, \"|\")", "read")
        .expect("read");
    engine.harvest();
    let config = engine.config();
    config
        .string("seen")
        .unwrap_or_default()
        .split('|')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_kind_of_event_arrives_in_lua_with_its_name_and_its_fields() {
    // The whole point of P2b: a watcher can see a turn begin and end, and what it is told is
    // true. One event and one shipped consumer was a callback, not a hook surface.
    let engine = watching();
    let mut registry = Registry::new();
    registry.watch(Box::new(magi_lua::tool::LuaWatch::new(Rc::clone(&engine))));

    registry.saw(&magi_tools::Event::Session {
        id: "s1",
        resumed: true,
    });
    registry.saw(&magi_tools::Event::TurnBegan {
        model: "openrouter/x",
    });
    registry.saw(&magi_tools::Event::Asked {
        verb: "run",
        about: "git status",
    });
    registry.saw(&magi_tools::Event::Compacted {
        dropped: 7,
        kept: 12,
    });
    registry.saw(&magi_tools::Event::TurnEnded {
        model: "openrouter/x",
        took_ms: 4,
        ok: true,
    });

    assert_eq!(
        seen(&engine),
        [
            "session.opened:s1",
            "turn.began:openrouter/x",
            "permission.asked:run",
            "context.compacted:7",
            "turn.ended:openrouter/x",
        ],
        "in order, with their fields"
    );
}

#[test]
fn a_watcher_that_raises_costs_itself_that_observation_and_nothing_else() {
    // The containment that makes watching after the fact safe. It held for tool events; the new
    // ones go through the same call, and this is what says so.
    let mut engine = Engine::new();
    engine
        .run(
            r#"
            __after = 0
            magi.watch("angry", { run = function() error("no") end })
            magi.watch("counting", { run = function() __after = __after + 1 end })
            "#,
            "test",
        )
        .expect("config");
    engine.harvest();
    let engine = Rc::new(RefCell::new(engine));

    let watcher = magi_lua::tool::LuaWatch::new(Rc::clone(&engine));
    watcher.saw(&magi_tools::Event::TurnBegan { model: "m" });

    let mut engine = engine.borrow_mut();
    engine.run("magi.after = __after", "read").expect("read");
    engine.harvest();
    assert_eq!(
        engine.config().number("after"),
        Some(1.0),
        "the second watcher still ran"
    );
}
