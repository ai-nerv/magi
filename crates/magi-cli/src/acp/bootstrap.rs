use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use agent_client_protocol_schema::v1::{LoadSessionRequest, McpServer, NewSessionRequest};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const SETTINGS_BYTES: usize = 64 * 1024;
pub(super) const READY: &[u8] = b"magi-acp-ready\n";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bootstrap {
    servers: Vec<Server>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    load: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Server {
    alias: String,
    name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

pub(super) fn validate(params: Value) -> Result<(NewSessionRequest, Bootstrap)> {
    let object = params
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected session parameters"))?;
    ensure!(
        object.keys().all(|key| matches!(
            key.as_str(),
            "cwd" | "mcpServers" | "additionalDirectories" | "_meta"
        )),
        "unsupported session setting"
    );
    ensure!(
        object.get("mcpServers").is_some_and(Value::is_array),
        "MCP servers must be an array"
    );
    for server in object
        .get("mcpServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let fields = server
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("invalid MCP declaration"))?;
        ensure!(
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "name" | "command" | "args" | "env" | "_meta")),
            "unsupported MCP transport or setting"
        );
    }
    let requested: NewSessionRequest = serde_json::from_value(params)?;
    ensure!(
        requested.cwd.is_absolute() && requested.cwd.as_os_str().as_encoded_bytes().len() <= 4096,
        "expected bounded absolute cwd"
    );
    ensure!(
        requested.additional_directories.is_empty(),
        "additional directories are not implemented"
    );
    ensure!(requested.mcp_servers.len() <= 16, "too many MCP servers");
    let prefix = magi_ipc::random_hex(8)?;
    let mut servers = Vec::new();
    for (index, server) in requested.mcp_servers.iter().enumerate() {
        let McpServer::Stdio(server) = server else {
            bail!("only stdio MCP is supported");
        };
        let mut env = BTreeMap::new();
        for entry in &server.env {
            ensure!(
                env.insert(entry.name.clone(), entry.value.clone())
                    .is_none(),
                "duplicate MCP environment variable"
            );
        }
        servers.push(Server {
            alias: format!("a{prefix}_{index:x}"),
            name: server.name.clone(),
            command: server
                .command
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("MCP command is not UTF8"))?
                .to_owned(),
            args: server.args.clone(),
            env,
        });
    }
    let bootstrap = Bootstrap {
        servers,
        load: None,
    };
    bootstrap.check()?;
    ensure!(
        serde_json::to_vec(&bootstrap)?.len() <= SETTINGS_BYTES,
        "MCP settings exceed byte limit"
    );
    Ok((requested, bootstrap))
}

pub(super) fn validate_load(mut params: Value) -> Result<(std::path::PathBuf, Bootstrap)> {
    let requested: LoadSessionRequest = serde_json::from_value(params.clone())?;
    let native = super::history::native(&requested.session_id.to_string())?;
    params
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("expected load parameters"))?
        .remove("sessionId");
    let (requested, mut bootstrap) = validate(params)?;
    bootstrap.load = Some(native);
    bootstrap.check()?;
    bootstrap.encode()?;
    Ok((requested.cwd, bootstrap))
}

impl Bootstrap {
    pub(crate) fn loading(&self) -> Option<&str> {
        self.load.as_deref()
    }

    fn check(&self) -> Result<()> {
        if let Some(load) = &self.load {
            super::history::external(load)?;
        }
        ensure!(self.servers.len() <= 16, "too many MCP servers");
        let mut names = BTreeSet::new();
        let mut aliases = BTreeSet::new();
        for server in &self.servers {
            ensure!(
                text(&server.name, 256) && !server.name.is_empty() && names.insert(&server.name),
                "invalid or duplicate MCP name"
            );
            ensure!(
                !server.alias.is_empty()
                    && server.alias.len() <= 20
                    && server
                        .alias
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && !server.alias.contains("__")
                    && !server.alias.starts_with('_')
                    && !server.alias.ends_with('_')
                    && aliases.insert(&server.alias),
                "invalid or duplicate MCP alias"
            );
            ensure!(
                text(&server.command, 4096) && Path::new(&server.command).is_absolute(),
                "MCP command must be absolute"
            );
            ensure!(
                server.args.len() <= 64 && server.args.iter().all(|arg| text(arg, 4096)),
                "MCP argument limit"
            );
            ensure!(server.env.len() <= 64, "MCP environment limit");
            for (key, value) in &server.env {
                ensure!(
                    !key.is_empty() && text(key, 128) && !key.contains('=') && text(value, 4096),
                    "invalid MCP environment"
                );
            }
        }
        Ok(())
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= SETTINGS_BYTES,
            "MCP settings exceed byte limit"
        );
        Ok(bytes)
    }

    pub(crate) fn merge(self, loaded: &mut Option<crate::config::Loaded>) -> Result<()> {
        self.check()?;
        if self.servers.is_empty() {
            return Ok(());
        }
        let loaded = loaded
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("MCP needs an installed Magi configuration"))?;
        let program = crate::config::roles::fills(loaded, "tools");
        let mut settings = loaded
            .config
            .get(&program)
            .cloned()
            .unwrap_or_else(|| json!({}));
        let table = settings
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("invalid tools configuration"))?;
        let servers = table
            .entry("mcp_servers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("invalid MCP configuration"))?;
        for server in &self.servers {
            ensure!(!servers.contains_key(&server.alias), "MCP alias collision");
        }
        for server in self.servers {
            servers.insert(server.alias, json!({"command":server.command,"args":server.args,"env":server.env,"needs":"run","network":false}));
        }
        loaded.config.settings.insert(program, settings);
        Ok(())
    }
}

fn text(value: &str, bytes: usize) -> bool {
    value.len() <= bytes && !value.contains('\0')
}

pub(crate) fn receive(parent: u32) -> Result<Bootstrap> {
    let expected = i32::try_from(parent)
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    ensure!(
        expected.is_some() && rustix::process::getppid() == expected,
        "ACP parent is gone"
    );
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::Kill))?;
    ensure!(
        rustix::process::getppid() == expected,
        "ACP parent changed during startup"
    );
    let file = std::fs::File::from(rustix::io::dup(rustix::stdio::stdin())?);
    rustix::stdio::dup2_stdin(std::fs::File::open("/dev/null")?)?;
    decode(file, 0)
}

pub(crate) fn ready(parent: u32) -> Result<bool> {
    let expected = i32::try_from(parent)
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    ensure!(expected.is_some(), "invalid ACP lifetime parent");
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::Term))?;
    if rustix::process::getppid() != expected {
        return Ok(false);
    }
    let mut output = std::io::stdout().lock();
    output.write_all(READY)?;
    output.flush()?;
    rustix::stdio::dup2_stdout(std::fs::OpenOptions::new().write(true).open("/dev/null")?)?;
    Ok(true)
}

fn decode(file: std::fs::File, links: u64) -> Result<Bootstrap> {
    let held = file.metadata()?;
    ensure!(
        held.is_file()
            && held.uid() == magi_ipc::uid()
            && held.mode() & 0o077 == 0
            && held.nlink() == links,
        "MCP startup file must be private and unaliased"
    );
    let mut bytes = Vec::new();
    file.take((SETTINGS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= SETTINGS_BYTES,
        "MCP settings exceed byte limit"
    );
    let value = super::json::parse(&bytes)?;
    let bootstrap: Bootstrap = serde_json::from_value(value)?;
    bootstrap.check()?;
    Ok(bootstrap)
}

#[cfg(test)]
mod tests;
