//! What the config API promises, checked against a real VM.

use crate::{Config, Engine};

/// Run one config chunk and harvest what it declared.
fn run(source: &str) -> Config {
    let mut engine = Engine::new();
    engine.run(source, "test.lua").expect("the config must run");
    engine.harvest();
    engine.config()
}

#[test]
fn a_setting_is_assigned_not_declared_in_a_table() {
    let config = run(r#"magi.model = "anthropic/claude-sonnet-4-5""#);
    assert_eq!(config.string("model"), Some("anthropic/claude-sonnet-4-5"));
}

#[test]
fn a_setting_the_config_never_mentions_is_absent_rather_than_defaulted() {
    let config = run("magi.model = 'x'");
    assert!(
        config.get("nothing_set_this").is_none(),
        "the host decides its own default"
    );
}

#[test]
fn nested_data_stays_nested() {
    // Rule 1 is about delivery: a tree is assigned as a tree, not flattened into statements.
    let config = run(r#"
        magi.keys = { submit = "enter", newline = "shift+enter" }
        "#);
    let keys = config.get("keys").expect("keys");
    assert_eq!(keys["submit"], "enter");
    assert_eq!(keys["newline"], "shift+enter");
}

#[test]
fn a_config_can_read_back_what_it_assigned() {
    let config = run(r#"
        magi.model = "a"
        if magi.model == "a" then magi.model = "b" end
        "#);
    assert_eq!(
        config.string("model"),
        Some("b"),
        "only the final value counts"
    );
}

#[test]
fn the_config_returns_nothing_and_may_branch_and_loop() {
    // Rule 3: a config is statements, so it can probe the machine it runs on. Written against
    // tools, which are the declarations magi actually keeps — this said `magi.provider` when
    // that registrar still existed, and passed just as well when nothing read what it stored.
    let mut engine = Engine::new();
    engine
        .run(
            r#"
        for _, id in ipairs({ "a", "b", "c" }) do
          magi.tool(id, { description = id, parameters = {}, run = function() end })
        end
        "#,
            "loop.lua",
        )
        .expect("run");
    assert_eq!(engine.tools().len(), 3);
}

#[test]
fn a_table_is_an_argument_not_a_fragment_to_merge() {
    // Rule 4: a setting arrives whole, with its nesting intact, rather than being merged key by
    // key into whatever was there before.
    let config = run(r#"
        magi.box = {
          name = "My vLLM box",
          base_url = "http://10.0.0.7:8000/v1",
          models = {
            { id = "Qwen/Qwen3-Coder-30B", context_window = 262144 },
          },
        }
        "#);
    let spec = config.get("box").expect("the setting");
    assert_eq!(spec["base_url"], "http://10.0.0.7:8000/v1");
    assert_eq!(spec["models"][0]["context_window"], 262144);
}

#[test]
fn a_list_table_becomes_an_array_and_a_keyed_one_an_object() {
    let config = run(r#"
        magi.list = { "a", "b" }
        magi.map = { a = 1 }
        "#);
    assert!(config.get("list").expect("list").is_array());
    assert!(config.get("map").expect("map").is_object());
}

#[test]
fn a_registrar_for_something_magi_does_not_own_says_who_does() {
    // All four of these stored what they were handed and nothing ever read it. A config that
    // declared a provider here worked exactly as well as one that declared nothing, and neither
    // magi nor the config author was told.
    let config = run(r#"
        magi.provider("evil", { base_url = "http://attacker.example" })
        magi.agent("worker", {})
        magi.mux("tmux", {})
        "#);
    let said = config.unkept.join("\n");
    assert!(said.contains("magi.provider(\"evil\")"), "{said}");
    assert!(
        said.contains("melchior"),
        "it names who does own it: {said}"
    );
    assert!(said.contains("hexe"), "and each names its own: {said}");
    assert_eq!(config.unkept.len(), 3, "one line per call: {said}");
}

#[test]
fn a_moved_registrar_keeps_nothing_it_was_handed() {
    // The point of the change. Silence would have been a lie either way; storing it was the
    // lie that looked like it worked.
    let config = run(r#"magi.provider("evil", { base_url = "http://attacker.example" })"#);
    assert!(
        !config.unkept.join("").contains("attacker.example"),
        "the message names the declaration, never repeats its contents"
    );
    assert!(
        config.get("evil").is_none(),
        "and it is not a setting either"
    );
}

#[test]
fn a_syntax_error_is_fatal_and_names_the_file() {
    let mut engine = Engine::new();
    let error = engine
        .run("magi.model = = ", "broken.lua")
        .expect_err("must fail");
    assert!(error.to_string().contains("broken.lua"), "{error}");
}

#[test]
fn a_raise_at_load_time_is_fatal_and_names_the_file() {
    let mut engine = Engine::new();
    let error = engine
        .run("error('deliberate')", "raising.lua")
        .expect_err("must fail");
    assert!(error.to_string().contains("raising.lua"), "{error}");
}

#[test]
fn a_registrar_without_a_name_is_refused() {
    let mut engine = Engine::new();
    assert!(engine.run("magi.provider({})", "bad.lua").is_err());
}

#[test]
fn a_config_may_probe_and_pcall_its_own_mistakes() {
    // A refused registration raises a string, so a config can survive its own bad input.
    let config = run(r#"
        local ok = pcall(function() magi.provider({}) end)
        magi.probed = ok
        magi.provider("good", {})
        magi.after = true
        "#);
    assert_eq!(config.get("probed"), Some(&serde_json::Value::Bool(false)));
    assert_eq!(
        config.get("after"),
        Some(&serde_json::Value::Bool(true)),
        "the config carried on"
    );
}

#[test]
fn later_files_win_over_earlier_ones() {
    let mut engine = Engine::new();
    engine
        .run("magi.model = 'machine'", "init.lua")
        .expect("run");
    engine
        .run("magi.model = 'project'", ".magi.lua")
        .expect("run");
    engine.harvest();
    assert_eq!(engine.config().string("model"), Some("project"));
}

#[test]
fn a_deeply_nested_table_is_refused_rather_than_overflowing() {
    let mut engine = Engine::new();
    // A setting, because that is the conversion that is still there: the registrars that used to
    // describe their argument as JSON no longer keep one, so the bound has to be tested where it
    // is actually applied.
    let deep = format!(
        "magi.deep = {}{}",
        "{ a = ".repeat(40),
        "1 ".to_owned() + &"}".repeat(40)
    );
    engine
        .run(&deep, "deep.lua")
        .expect("the assignment itself is fine");
    engine.harvest();
    let config = engine.config();
    assert!(config.get("deep").is_none(), "a bound, not a crash");
    assert!(
        config.unkept.iter().any(|said| said.contains("magi.deep")),
        "and the config author is told: {:?}",
        config.unkept
    );
}

#[test]
fn real_lua_is_available_not_a_subset() {
    // The point of a full VM: string patterns, closures, varargs, table library.
    let config = run(r#"
        local function join(...)
          return table.concat({ ... }, "-")
        end
        magi.model = join("anthropic", "claude"):gsub("claude", "sonnet")
        "#);
    assert_eq!(config.string("model"), Some("anthropic-sonnet"));
}

#[test]
fn a_project_config_is_applied_after_a_machine_one() {
    let paths = crate::search_paths();
    assert!(paths.last().expect("a path").ends_with(".magi.lua"));
}
