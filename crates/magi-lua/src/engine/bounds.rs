//! What a chunk cannot make a VM hold or run.

use super::Engine;

#[test]
fn no_route_past_the_guard_enrols_an_object_for_a_finalizer() {
    let mut engine = Engine::new();
    engine.lua.gc_collect();
    let before = engine.lua.total_memory();
    engine
        .run(
            "local gc = { __gc = function() end }\n\
             local routes = {\n\
               function(t) return setmetatable(t, gc) end,\n\
               function(t) return rawget(_G, 'setmetatable')(t, gc) end,\n\
               function(t) return debug.setmetatable(t, gc) end,\n\
               function(t) local _, set = debug.getupvalue(setmetatable, 1) return set(t, gc) end,\n\
             }\n\
             local took = 0\n\
             for _, route in ipairs(routes) do\n\
               for _ = 1, 5000 do\n\
                 if pcall(route, { string.rep('x', 1024) }) then took = took + 1 end\n\
               end\n\
             end\n\
             for _ = 1, 5000 do\n\
               local late = {}\n\
               setmetatable({ string.rep('y', 1024) }, late)\n\
               late.__gc = gc.__gc\n\
             end\n\
             magi.took = took",
            "enrol.lua",
        )
        .expect("runs");
    engine.harvest();
    assert_eq!(engine.config().number("took"), Some(0.0));
    engine.lua.gc_collect();
    engine.lua.gc_collect();
    let after = engine.lua.total_memory();
    assert!(
        after < before + (1 << 20),
        "{before} bytes before, {after} after"
    );
}
