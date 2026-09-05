//! `magi ext lua` — a second peer, so the protocol has more than one implementation.
//!
//! A protocol with one implementation is a function call with extra steps: nothing forces the
//! host to say what it means, because the only peer is the one that was written alongside it.
//! This is the second, and it is deliberately unlike the first — a different language for the
//! tool body, a different lifecycle, and one thing it cannot do at all.
//!
//! **It cannot be interrupted.** A Lua body runs to completion inside a stackless VM; there is
//! no point between entering it and leaving it at which a `Cancel` could be noticed. So this
//! peer does not answer one, and the host kills it after the grace period. That is not a gap
//! being papered over — it is what a second peer is for. A boundary that only works for peers
//! that can do everything is not a boundary, and the host's timeout is load-bearing precisely
//! because a peer like this exists.
//!
//! **It runs a file the host named.** Nothing is discovered: the config says which file, and a
//! file the config did not name is never loaded.

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

    // One `Declare` per tool, and the file may hold several. This is the thing a config cannot
    // do for a peer: what a file offers is known by running it, and only the peer runs it.
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
        // A peer that answers no calls would sit there being connected to, and the mistake
        // would present as a tool that is never offered rather than as a file that is wrong.
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
