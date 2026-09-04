//! `magi tools` — what the model can call, and how each one is reached.

use magi_tools::{Registry, Tool};

/// Print the registry, transport and all.
///
/// The transport is shown because it is the thing a person needs to know: a Lua tool runs in
/// this process, a process tool is a peer with its own life, and which one a tool is decides
/// what happens when it misbehaves.
pub fn print() -> Result<(), magi_lua::LuaError> {
    let loaded = crate::config::load()?;
    let mut engine = magi_lua::Engine::new();
    engine.install_clients(&loaded.clients);
    for (name, source) in &loaded.tools {
        engine.run(source, name)?;
    }
    let declared = engine.tools();

    let engine = std::rc::Rc::new(std::cell::RefCell::new(engine));
    let mut registry = Registry::new();
    // casper first, so anything nearer wins — the same order the worker uses, because a listing
    // that disagreed with what a session runs would be answering a different question.
    //
    // Nobody to ask: `magi tools` lists what exists and runs nothing, so a tool that would have
    // stopped to ask never gets the chance to.
    let mut from_casper = std::collections::BTreeSet::new();
    for tool in magi_tools::casper::CasperTool::all(
        magi_tools::casper::CASPER,
        std::sync::Arc::new(magi_tools::question::Unanswered),
    ) {
        from_casper.insert(tool.name().to_owned());
        registry.register(Box::new(tool));
    }
    magi_tools::builtin::install(&mut registry);
    magi_lua::tool::install(
        std::rc::Rc::clone(&engine),
        &mut registry,
        &Default::default(),
    );
    // A name a config declared for itself is that config's, however far it also travelled.
    for tool in registry.declarations() {
        if from_casper.contains(&tool.name) && declared.iter().any(|(name, _)| *name == tool.name) {
            from_casper.remove(&tool.name);
        }
    }
    // Asked rather than assumed. `magi tools` answers "what can the model call", and the only
    // thing that knows what a peer offers is the peer.
    registry.probe(&magi_tools::ops::Real::new(
        std::env::current_dir().unwrap_or_default(),
    ));

    for tool in registry.declarations() {
        let transport = if from_casper.contains(&tool.name) {
            "casper"
        } else {
            declared
                .iter()
                .find(|(name, _)| *name == tool.name)
                .and_then(|(_, spec)| spec.get("transport"))
                .and_then(|t| t.get("kind"))
                .and_then(|k| k.as_str())
                .unwrap_or("builtin")
        };
        println!(
            "{:<10} {:<9} {}",
            tool.name,
            transport,
            first_line(&tool.description)
        );
    }
    Ok(())
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
