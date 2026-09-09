//! `magi ext lua` — a second peer, so the protocol has more than one implementation. It cannot be
//! interrupted: a Lua body runs to completion inside a stackless VM, so this peer never answers a
//! `Cancel` and the host kills it after the grace period. It runs the file the config named and
//! discovers nothing.

use anyhow::{Context, Result};
use magi_ipc::blocking::{FrameReader, FrameWriter};
use magi_proto::{ToolReport, ToolRequest};
use std::path::Path;

/// Load `path` and serve whatever it declared.
pub fn run(path: &Path) -> Result<()> {
    let mut engine = magi_lua::Engine::new();
    engine
        .run_file(path)
        .with_context(|| format!("loading {}", path.display()))?;

    let mut reader = FrameReader::new(std::io::stdin());
    let mut writer = FrameWriter::new(std::io::stdout());

    // One `Declare` per tool, and the file may hold several: what a file offers is known by
    // running it, and only the peer runs it.
    let declared = engine.tools();
    if declared.is_empty() {
        anyhow::bail!(
            "{} declared no tools; a peer with nothing to offer is a configuration mistake",
            path.display()
        );
    }
    for (name, spec) in &declared {
        writer.write_blocking(&ToolReport::Declare {
            name: name.clone(),
            description: spec
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_owned(),
            parameters: spec
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
        })?;
    }

    loop {
        let request = match reader.read_blocking::<ToolRequest>() {
            Ok(request) => request,
            // The host went away. Nothing to report to, so leave quietly.
            Err(_) => return Ok(()),
        };
        match request {
            ToolRequest::Call {
                id,
                name,
                arguments,
            } => {
                let (output, is_error) = match engine.call_tool(&name, &arguments) {
                    Some(value) => (
                        value
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        value
                            .get("is_error")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    ),
                    None => (format!("{name} is not a tool this peer offers"), true),
                };
                writer.write_blocking(&ToolReport::Result {
                    id,
                    output,
                    is_error,
                })?;
            }
            // Read, understood, and impossible to act on: see the note at the top. Answering
            // anyway would be a lie, and the host has a timeout for exactly this.
            ToolRequest::Cancel { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::{Scratch, ScratchFile};

    fn peer_file(name: &str, source: &str) -> ScratchFile {
        let path = Scratch::file("magi-extlua", name, &format!("{name}.lua"));
        std::fs::write(&path, source).expect("write");
        path
    }

    #[test]
    fn a_file_declaring_nothing_is_refused_rather_than_served() {
        // A peer that answers no calls presents as a tool never offered rather than a wrong file.
        let path = peer_file("empty", "local unused = 1\n");
        let why = run(&path).expect_err("a peer with no tools is an error");
        assert!(why.to_string().contains("declared no tools"), "{why}");
    }

    #[test]
    fn a_file_that_will_not_load_names_itself() {
        let path = peer_file("broken", "this is not lua\n");
        let why = run(&path).expect_err("a broken file is an error");
        assert!(why.to_string().contains("broken.lua"), "{why}");
    }
}
