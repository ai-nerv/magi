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
    // Rule 3: a config is statements, so it can probe the machine it runs on.
    let config = run(r#"
        for _, id in ipairs({ "a", "b", "c" }) do
          magi.provider(id, { name = id, api = "openai-completions" })
        end
        "#);
    assert_eq!(config.all("provider").len(), 3);
}

#[test]
fn registration_is_keyed_so_re_running_replaces_rather_than_appends() {
    // Rule 2, map form. A config that loops over a directory of machines is idempotent.
    let config = run(r#"
        magi.provider("box", { name = "first" })
        magi.provider("box", { name = "second" })
        "#);
    let registered = config.all("provider");
    assert_eq!(registered.len(), 1, "one identity, one entry");
    assert_eq!(registered[0].1["name"], "second", "the last wins");
}

#[test]
fn declaration_order_is_kept() {
    let config = run(r#"
        magi.provider("z", {})
        magi.provider("a", {})
        magi.provider("m", {})
        "#);
    let ids: Vec<&str> = config.all("provider").iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, ["z", "a", "m"], "a picker's list must not reshuffle");
}

#[test]
fn a_table_is_an_argument_not_a_fragment_to_merge() {
    // Rule 4: a provider arrives whole, with its nested model list intact.
    let config = run(r#"
        magi.provider("my-vllm", {
          name = "My vLLM box",
          api = "openai-completions",
          base_url = "http://10.0.0.7:8000/v1",
          models = {
            { id = "Qwen/Qwen3-Coder-30B", context_window = 262144 },
          },
        })
        "#);
    let (id, spec) = config.all("provider")[0];
    assert_eq!(id, "my-vllm");
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
fn each_registrar_keeps_its_own_namespace() {
    let config = run(r#"
        magi.provider("same", { which = "provider" })
        magi.agent("same", { which = "agent" })
        "#);
    assert_eq!(config.all("provider")[0].1["which"], "provider");
    assert_eq!(config.all("agent")[0].1["which"], "agent");
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
        "#);
    assert_eq!(config.get("probed"), Some(&serde_json::Value::Bool(false)));
    assert_eq!(config.all("provider").len(), 1, "the config carried on");
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
    let deep = format!(
        "magi.provider('deep', {}{})",
        "{ a = ".repeat(40),
        "1 ".to_owned() + &"}".repeat(40)
    );
    assert!(
        engine.run(&deep, "deep.lua").is_err(),
        "a bound, not a crash"
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
