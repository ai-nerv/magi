//! `magi doctor` — what a session here would be made of, without starting one: which config files
//! were read, which of their lines magi kept, what the registry holds, and which siblings are
//! there. The composition is built by the same [`magi_lua::tool::assemble`] a session uses, with
//! nobody to ask, no screen to lend, and the working directory rather than a gated root.

/// Print the composition of a session in this directory. Never fails: a configuration that will not
/// load is the loudest thing this can report, not a reason to stop.
pub fn print() {
    // Everything below still holds when this fails: the builtins are compiled in and the siblings
    // are on `$PATH` or are not.
    let (loaded, refused) = match crate::config::load() {
        Ok(loaded) => (loaded, None),
        Err(why) => (nothing_loaded(), Some(why.to_string())),
    };

    heading("configuration");
    match crate::config::config_dir() {
        Some(dir) => row("directory", &dir.display().to_string()),
        None => row(
            "directory",
            "none: neither $XDG_CONFIG_HOME nor $HOME is set",
        ),
    }
    if let Some(why) = &refused {
        row("state", &format!("will not load: {why}"));
    }
    row("tool files", &named(&loaded.tools));
    row("client libraries", &named(&loaded.clients));

    heading("settings");
    let environ = crate::config::environ(&loaded);
    row(
        "model",
        loaded
            .config
            .string("model")
            .unwrap_or("(melchior's default)"),
    );
    row(
        "confine",
        if loaded.config.boolean("confine").unwrap_or(false) {
            "on"
        } else {
            "off"
        },
    );
    row(
        "standing grants",
        &crate::config::granted(&loaded).len().to_string(),
    );
    row(
        "environment",
        &if environ.is_empty() {
            "(none)".to_owned()
        } else {
            environ.keys().cloned().collect::<Vec<_>>().join(" ")
        },
    );

    // What a config said that magi did not keep. Printed here as well as at load, because this
    // is the command a person runs when something they wrote did nothing.
    if !loaded.config.unkept.is_empty() {
        heading("not kept");
        for said in &loaded.config.unkept {
            println!("  {said}");
        }
    }

    heading("tools");
    let mut engine = magi_lua::Engine::new();
    engine.install_clients(&loaded.clients);
    for (name, source) in &loaded.tools {
        if let Err(why) = engine.run(source, name) {
            row(name, &format!("will not run: {why}"));
        }
    }
    let declared = engine.tools();
    let engine = std::rc::Rc::new(std::cell::RefCell::new(engine));
    let (registry, from_casper) = magi_lua::tool::assemble(
        std::rc::Rc::clone(&engine),
        std::sync::Arc::new(magi_tools::question::Unanswered),
        std::sync::Arc::new(magi_tools::holding::Screenless),
        &environ,
        crate::config::casper_pin(&loaded).as_deref(),
        &crate::config::casper_configure(&loaded),
    );
    registry.probe(&magi_tools::ops::Real::new(
        std::env::current_dir().unwrap_or_default(),
    ));

    for tool in registry.declarations() {
        let source = if from_casper.contains(&tool.name) {
            "casper".to_owned()
        } else if declared.iter().any(|(name, _)| *name == tool.name) {
            "config".to_owned()
        } else {
            "builtin".to_owned()
        };
        println!("  {:<10} {source}", tool.name);
        // Only a peer has anything more to say: a command line, and the environment it was built
        // with. That last one is where `magi tools` and a session used to disagree.
        if let Some(built) = registry.get(&tool.name) {
            for (what, said) in built.composition() {
                println!("    {what:<10} {said}");
            }
        }
    }

    heading("siblings");
    for (name, what) in SIBLINGS {
        row(name, &sibling(name, what));
    }
}

/// A configuration that is not there, so the rest of the report can still be made.
fn nothing_loaded() -> crate::config::Loaded {
    crate::config::Loaded {
        config: magi_lua::Config::default(),
        tools: Vec::new(),
        clients: Vec::new(),
    }
}

/// The programs a session reaches for, and what each is for. Named rather than discovered, so the
/// report answers "what is missing" instead of "what is installed".
const SIBLINGS: &[(&str, &str)] = &[
    ("casper", "tools"),
    ("melchior", "the model"),
    ("balthasar", "memory"),
];

/// Whether `name` is installed, and whether it actually answers: a program on `$PATH` is not a
/// running one, and a socket that accepts is not one that answers.
fn sibling(name: &str, what: &str) -> String {
    let Some(path) = which(name) else {
        return format!("not installed — no {what}");
    };
    let at = path.display().to_string();
    match name {
        // Served on a socket, and the socket is the thing that lies.
        "balthasar" => match magi_ipc::family::blocking::Family::find() {
            Ok(_) => format!("{at} — answering"),
            Err(why) => format!("{at} — installed, but {why}"),
        },
        // Asked the way magi asks them: one listing verb, whose emptiness is itself the answer.
        "casper" => match magi_tools::casper::cards_from(name).len() {
            0 => format!("{at} — installed, but offers no tools"),
            n => format!("{at} — {n} tools"),
        },
        _ => match answers_models(name) {
            Some(n) => format!("{at} — {n} models"),
            None => format!("{at} — installed, but would not answer `models`"),
        },
    }
}

/// How many models `name` offers, or nothing when it would not say.
fn answers_models(name: &str) -> Option<usize> {
    let out = std::process::Command::new(name)
        .arg("models")
        .arg("--json")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(reply.get("result")?.as_array()?.len())
}

/// The first `name` on `$PATH`.
fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// One `name: value` line, aligned.
fn row(name: &str, value: &str) {
    println!("  {name:<18} {value}");
}

/// A section title.
fn heading(title: &str) {
    println!("\n{title}");
}

/// The names of a set of config files, or a note that there are none.
fn named(files: &[(String, String)]) -> String {
    if files.is_empty() {
        return "(none)".to_owned();
    }
    files
        .iter()
        .map(|(name, _)| name.rsplit('/').next().unwrap_or(name).to_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{named, which};

    #[test]
    fn a_missing_program_is_not_found() {
        assert!(which("magi-no-such-program-anywhere").is_none());
    }

    #[test]
    fn a_file_is_named_by_its_basename() {
        let files = vec![
            ("clients/oslo.lua".to_owned(), String::new()),
            ("tools.lua".to_owned(), String::new()),
        ];
        assert_eq!(named(&files), "oslo.lua tools.lua");
        assert_eq!(named(&[]), "(none)");
    }
}
