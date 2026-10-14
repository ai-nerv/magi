//! What a chunk cannot make a VM hold or run.

use super::{Engine, MEMORY, PROJECT_MEMORY};

/// The shipped configuration and example plugins, by name.
const SHIPPED: &[(&str, &str)] = &[
    ("init.lua", include_str!("../../../../config/init.lua")),
    ("tools.lua", include_str!("../../../../config/tools.lua")),
    (
        "ripgrep.lua",
        include_str!("../../../../examples/plugin/ripgrep.lua"),
    ),
    (
        "status-line.lua",
        include_str!("../../../../examples/plugin/status-line.lua"),
    ),
];

#[test]
fn the_shipped_config_and_plugins_fit_far_under_the_ceiling() {
    let mut engine = Engine::new();
    for (name, source) in SHIPPED {
        engine.run(source, name).expect(name);
    }
    engine.lua.gc_collect();
    let held = engine.lua.total_memory();
    assert_eq!(engine.lua.memory_limit(), Some(MEMORY));
    assert!(held < MEMORY / 256, "{held} bytes held");
}

#[test]
fn a_chunk_over_the_ceiling_is_stopped_and_the_vm_recovers() {
    let mut engine = Engine::new();
    engine.lua.set_memory_limit(Some(8 << 20));
    let why = engine
        .run(
            "local t = {} for i = 1, 1e7 do t[i] = ('x'):rep(64) .. i end",
            "plugin/hog.lua",
        )
        .expect_err("a hog finished");
    let said = why.to_string();
    assert!(said.contains("plugin/hog.lua"), "{said}");
    assert!(said.contains("8 MiB of memory"), "{said}");
    engine
        .run("magi.after = 1", "after.lua")
        .expect("the VM runs again once the hog is gone");
}

#[test]
fn garbage_is_collected_before_the_ceiling_is_judged() {
    let mut engine = Engine::new();
    engine.lua.set_memory_limit(Some(8 << 20));
    engine
        .run(
            "collectgarbage('stop') for i = 1, 50000 do local s = ('x'):rep(1024) .. i end",
            "churn.lua",
        )
        .expect("garbage alone is not held");
}

#[test]
fn a_tool_that_hoards_memory_fails_with_why_and_the_engine_still_answers() {
    let mut engine = Engine::new();
    engine
        .run(
            "magi.tool('hoard', { run = function()\n\
               local kept = {} for i = 1, 1e7 do kept[i] = ('x'):rep(64) .. i end\n\
             end })\n\
             magi.tool('ok', { run = function() return 'fine' end })",
            "tools.lua",
        )
        .expect("declares");
    engine.lua.set_memory_limit(Some(8 << 20));
    let null = serde_json::Value::Null;
    let hoarded = engine.call_tool("hoard", &null).expect("an answer");
    assert_eq!(hoarded["is_error"], true, "{hoarded}");
    let said = hoarded["content"].as_str().unwrap_or_default();
    assert!(
        said.contains("hoard") && said.contains("MiB of memory"),
        "{said}"
    );
    let fine = engine.call_tool("ok", &null).expect("an answer");
    assert_eq!(fine["content"], "fine", "{fine}");
}

#[test]
fn a_project_file_at_its_largest_fits_its_small_ceiling() {
    let mut source = String::new();
    while source.len() < 256 * 1024 - 64 {
        let at = source.len();
        source.push_str(&format!(
            "magi.ui.k{at} = {{ a = 1, b = 'two', c = {{ 3, 4 }} }}\n"
        ));
    }
    let mut project = Engine::project(&serde_json::Map::new());
    assert_eq!(project.lua.memory_limit(), Some(PROJECT_MEMORY));
    project.run(&source, ".magi.lua").expect("fits");
    project.lua.gc_collect();
    let held = project.lua.total_memory();
    assert!(held < PROJECT_MEMORY / 2, "{held} bytes held");
    let why = project
        .run("local t = {} for i = 1, 1e7 do t[i] = {} end", ".magi.lua")
        .expect_err("a hog finished");
    assert!(why.to_string().contains("16 MiB of memory"), "{why}");
}

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
