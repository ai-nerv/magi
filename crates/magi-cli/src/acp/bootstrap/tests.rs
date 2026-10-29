#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use magi_model::scratch::Scratch;
use std::io::Write;

fn apply(path: &Path, loaded: &mut Option<crate::config::Loaded>) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)?;
    decode(file, 1)?.merge(loaded)
}

fn declaration() -> Value {
    json!({"cwd":"/work","mcpServers":[{"name":"Editor filesystem λ","command":"/bin/server","args":["--literal", "$(not executed)"],"env":[{"name":"TOKEN","value":"private-value"}]}]})
}

fn configured() -> Option<crate::config::Loaded> {
    Some(crate::config::Loaded {
        config: magi_lua::Config::default(),
        tools: Vec::new(),
        clients: Vec::new(),
    })
}

#[test]
fn persistent_load_validates_identity_and_preserves_private_mcp_handoff() {
    let mut params = declaration();
    params["sessionId"] = json!(super::super::history::external("run@child").expect("identity"));
    let (cwd, bootstrap) = validate_load(params).expect("load");
    assert_eq!(cwd, Path::new("/work"));
    assert_eq!(bootstrap.loading(), Some("run@child"));
    let encoded = bootstrap.encode().expect("bounded handoff");
    let decoded: Bootstrap = serde_json::from_slice(&encoded).expect("handoff");
    decoded.check().expect("validated handoff");
    assert_eq!(decoded.loading(), Some("run@child"));
    assert_eq!(decoded.servers[0].args[1], "$(not executed)");
    assert!(validate(json!({"cwd":"/work","mcpServers":[],"load":"run"})).is_err());
}

#[test]
fn persistent_load_rejects_ambiguous_ids_and_unsupported_settings() {
    for id in [
        json!(null),
        json!(1),
        json!(""),
        json!("run"),
        json!("magi-v1-FF"),
        json!("magi-v1-00"),
        json!("magi-v1-1"),
        json!("x".repeat(1033)),
    ] {
        assert!(validate_load(json!({"cwd":"/work","mcpServers":[],"sessionId":id})).is_err());
    }
    for (key, value) in [
        ("mcpServers", json!(null)),
        ("cwd", json!("relative")),
        ("systemPrompt", json!("override")),
        ("additionalDirectories", json!(["/extra"])),
        ("load", json!("other")),
    ] {
        let mut params = json!({"cwd":"/work","mcpServers":[],"sessionId":"magi-v1-41"});
        params[key] = value;
        assert!(validate_load(params).is_err());
    }
    for native in ["", "x\n", &"x".repeat(513)] {
        let bootstrap: Bootstrap =
            serde_json::from_value(json!({"servers":[],"load":native})).expect("decode");
        assert!(bootstrap.check().is_err());
    }
}

#[test]
fn bounded_mcp_handoff_preserves_literals_names_and_native_policy() {
    let (_, bootstrap) = validate(declaration()).expect("valid");
    let alias = bootstrap.servers[0].alias.clone();
    assert!(alias.len() <= 20);
    assert_eq!(bootstrap.servers[0].name, "Editor filesystem λ");
    let mut loaded = configured();
    loaded.as_mut().expect("loaded").config.settings.insert(
        "casper".into(),
        json!({"custom":true,"mcp_servers":{"existing":{"command":"/installed/server"}}}),
    );
    bootstrap.merge(&mut loaded).expect("merge");
    let value = loaded.expect("loaded").config.settings;
    assert_eq!(value["casper"]["custom"], true);
    assert_eq!(
        value["casper"]["mcp_servers"]["existing"]["command"],
        "/installed/server"
    );
    let added = &value["casper"]["mcp_servers"][&alias];
    assert_eq!(added["args"][1], "$(not executed)");
    assert_eq!(added["env"]["TOKEN"], "private-value");
    assert_eq!(added["needs"], "run");
    assert_eq!(added["network"], false);
}

#[test]
fn mcp_admission_rejects_unknown_effects_duplicates_and_bounds() {
    for change in [
        json!({"type":"http","name":"x","url":"https://example.invalid"}),
        json!({"name":"x","command":"relative","args":[],"env":[]}),
        json!({"name":"x","command":"/bin/server","args":[],"env":[],"network":true}),
        json!({"name":"x","command":"/bin/server","args":[],"env":[{"name":"A","value":"1"},{"name":"A","value":"2"}]}),
        json!({"name":"x","command":"/bin/server","args":["\u{0}"],"env":[]}),
    ] {
        assert!(validate(json!({"cwd":"/work","mcpServers":[change]})).is_err());
    }
    let mut duplicate = declaration();
    duplicate["mcpServers"] = json!([
        duplicate["mcpServers"][0].clone(),
        duplicate["mcpServers"][0].clone()
    ]);
    assert!(validate(duplicate).is_err());
    let mut excessive = declaration();
    excessive["mcpServers"][0]["args"] = json!(["x".repeat(4097)]);
    assert!(validate(excessive).is_err());
    for params in [
        json!({"cwd":"relative","mcpServers":[]}),
        json!({"cwd":"/work","mcpServers":[],"systemPrompt":"override"}),
        json!({"cwd":"/work","mcpServers":[],"additionalDirectories":["/extra"]}),
    ] {
        assert!(validate(params).is_err());
    }
}

#[test]
fn mcp_alias_collision_does_not_overwrite_existing_configuration() {
    let (_, bootstrap) = validate(declaration()).expect("valid");
    let alias = bootstrap.servers[0].alias.clone();
    let mut loaded = configured();
    let prior = json!({"mcp_servers":{alias:{"command":"/installed"}}});
    loaded
        .as_mut()
        .expect("loaded")
        .config
        .settings
        .insert("casper".into(), prior.clone());
    assert!(bootstrap.merge(&mut loaded).is_err());
    assert_eq!(loaded.expect("loaded").config.settings["casper"], prior);
}

#[test]
fn private_bootstrap_rejects_links_permissions_and_truncated_payloads() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = Scratch::new("acp-bootstrap", "private");
    let path = root.join("settings");
    let (_, bootstrap) = validate(declaration()).expect("valid");
    magi_ipc::private_file(&path)
        .expect("private")
        .write_all(&bootstrap.encode().expect("encode"))
        .expect("write");
    assert!(apply(&path, &mut configured()).is_ok());
    let link = root.join("link");
    symlink(&path, &link).expect("symlink");
    assert!(apply(&link, &mut configured()).is_err());
    std::fs::hard_link(&path, root.join("hard")).expect("hardlink");
    assert!(apply(&path, &mut configured()).is_err());
    std::fs::remove_file(root.join("hard")).expect("unlink");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(apply(&path, &mut configured()).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    std::fs::write(
        &path,
        format!("{{\"servers\":[]}}{}", " ".repeat(SETTINGS_BYTES)),
    )
    .expect("write");
    assert!(apply(&path, &mut configured()).is_err());
}

#[test]
fn private_bootstrap_rejects_fifo_without_waiting_for_a_writer() {
    let root = Scratch::new("acp-bootstrap", "fifo");
    let path = root.join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("fifo");
    assert!(apply(&path, &mut configured()).is_err());
}
