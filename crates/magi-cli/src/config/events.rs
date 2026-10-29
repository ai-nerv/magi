use super::Loaded;

pub(super) fn harvest(
    engine: &mut magi_lua::Engine,
    launch: &std::path::Path,
) -> Result<magi_lua::Config, magi_lua::LuaError> {
    let selected = engine
        .assigned()
        .into_iter()
        .find(|(name, _)| name == "events");
    let value = match selected {
        None => None,
        Some((_, Some(serde_json::Value::Bool(value)))) => Some(value),
        Some(_) => {
            return Err(magi_lua::LuaError::Runtime {
                file: "configuration".into(),
                message: "magi.events must be a boolean".into(),
            });
        }
    };
    let mut config = super::workspace::harvest(engine, launch)?;
    config.settings.remove("events");
    if let Some(value) = value {
        config.settings.insert("events".into(), value.into());
    }
    Ok(config)
}

pub fn enabled(loaded: Option<&Loaded>) -> anyhow::Result<bool> {
    match loaded.and_then(|loaded| loaded.config.get("events")) {
        None => Ok(false),
        Some(serde_json::Value::Bool(enabled)) => Ok(*enabled),
        Some(_) => anyhow::bail!("magi.events must be a boolean"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_collection_requires_explicit_boolean_opt_in() {
        assert!(!enabled(None).expect("default"));
        for (source, expected) in [
            ("", Some(false)),
            ("magi.events = false", Some(false)),
            ("magi.events = true", Some(true)),
            ("magi.events = 'true'", None),
            ("magi.events = {}", None),
        ] {
            let mut engine = magi_lua::Engine::new();
            engine.run(source, "test").expect("config");
            engine.harvest();
            let loaded = Loaded {
                config: engine.config(),
                tools: Vec::new(),
                clients: Vec::new(),
            };
            assert_eq!(enabled(Some(&loaded)).ok(), expected);
        }
    }

    #[test]
    fn final_assignment_cannot_retain_a_stale_enabled_value() {
        for (last, expected) in [
            ("nil", Some(false)),
            ("false", Some(false)),
            ("true", Some(true)),
            ("function() end", None),
            ("{}", None),
            ("1", None),
            ("'true'", None),
        ] {
            let mut engine = magi_lua::Engine::new();
            engine
                .run("magi.events = true", "machine")
                .expect("initial");
            engine.harvest();
            engine
                .run(&format!("magi.events = {last}"), "override")
                .expect("last");
            let value = harvest(&mut engine, std::path::Path::new("/launch"))
                .ok()
                .map(|config| config.boolean("events").unwrap_or(false));
            assert_eq!(value, expected, "{last}");
        }
    }
}
