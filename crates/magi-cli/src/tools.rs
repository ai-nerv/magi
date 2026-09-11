//! `magi tools` — what the model can call, and how each one is reached.

use crate::verbs::As;
use magi_ipc::family::Reply;

/// One entry: what the model calls it, the transport that carries it, and what it is for. A Lua
/// tool runs in this process, a process tool is a peer with its own life, and which it is decides
/// what happens when it misbehaves.
struct Listed {
    name: String,
    transport: String,
    description: String,
}

/// Print the registry, transport and all.
pub fn print(how: As) -> Result<(), magi_lua::LuaError> {
    let listed = registry()?;
    if how.framed() {
        let rows = listed
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "transport": tool.transport,
                    "description": tool.description,
                })
            })
            .collect();
        crate::verbs::say(&Reply::rows(rows), how);
        return Ok(());
    }
    for tool in &listed {
        println!(
            "{:<10} {:<9} {}",
            tool.name,
            tool.transport,
            first_line(&tool.description)
        );
    }
    Ok(())
}

/// The registry a session would build, from the one place that knows how.
fn registry() -> Result<Vec<Listed>, magi_lua::LuaError> {
    let loaded = crate::config::load()?;
    let mut engine = magi_lua::Engine::new();
    engine.install_clients(&loaded.clients);
    for (name, source) in &loaded.tools {
        engine.run(source, name)?;
    }
    let declared = engine.tools();

    let engine = std::rc::Rc::new(std::cell::RefCell::new(engine));
    // Nobody to ask and no screen to lend: `magi tools` lists what exists and runs nothing, so a
    // tool that would have stopped to ask never gets the chance to.
    let tooling = crate::config::tooling(&loaded);
    let (registry, supplied) = magi_lua::tool::assemble(
        std::rc::Rc::clone(&engine),
        std::sync::Arc::new(magi_tools::question::Unanswered),
        std::sync::Arc::new(magi_tools::holding::Screenless),
        &crate::config::environ(&loaded),
        &tooling,
    );
    // Asked rather than assumed: the only thing that knows what a peer offers is the peer.
    // Through plain `Ops` at the working directory, since a listing acts on nothing.
    registry.probe(&magi_tools::ops::Real::new(
        std::env::current_dir().unwrap_or_default(),
    ));

    Ok(registry
        .declarations()
        .iter()
        .map(|tool| Listed {
            name: tool.name.clone(),
            transport: if supplied.contains(&tool.name) {
                tooling.program.clone()
            } else {
                declared
                    .iter()
                    .find(|(name, _)| *name == tool.name)
                    .and_then(|(_, spec)| spec.get("transport"))
                    .and_then(|t| t.get("kind"))
                    .and_then(|k| k.as_str())
                    .unwrap_or("builtin")
                    .to_owned()
            },
            description: tool.description.clone(),
        })
        .collect())
}

/// The first line of a description, for a listing.
fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(70)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::first_line;

    #[test]
    fn a_description_is_reduced_to_its_first_line() {
        assert_eq!(first_line("\nfirst\nsecond\n"), "first");
        assert_eq!(first_line(&"x".repeat(200)).len(), 70);
    }
}
