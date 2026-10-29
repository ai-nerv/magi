use magi_lua::{Config, Engine, LuaError};
use std::path::Path;

const SETTING: &str = "workflow_workspace";

pub(super) fn harvest(engine: &mut Engine, launch: &Path) -> Result<Config, LuaError> {
    let selected = engine
        .assigned()
        .into_iter()
        .find(|(name, _)| name == SETTING);
    let value = match selected {
        None => None,
        Some((_, Some(serde_json::Value::String(path)))) if !path.is_empty() => {
            magi_host::skills::Roots::new(launch, Some(Path::new(&path)), launch)
                .map_err(|why| invalid(&why.to_string()))?;
            Some(serde_json::Value::String(path))
        }
        Some(_) => return Err(invalid("expected a nonempty path string or nil")),
    };
    engine.harvest();
    let mut config = engine.config();
    config.settings.remove(SETTING);
    if let Some(value) = value {
        config.settings.insert(SETTING.into(), value);
    }
    Ok(config)
}

fn invalid(message: &str) -> LuaError {
    LuaError::Runtime {
        file: "configuration".into(),
        message: format!("magi.{SETTING}: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(source: &str) -> Result<Config, LuaError> {
        let mut engine = Engine::new();
        engine.run(source, "init.lua")?;
        harvest(&mut engine, Path::new("/launch/nested"))
    }

    #[test]
    fn absent_setting_stays_absent_and_paths_remain_literal() {
        assert!(configured("").expect("default").get(SETTING).is_none());
        for path in ["/selected/worktree", "selected", ".", "a path/$LITERAL"] {
            let source = format!("magi.workflow_workspace = {path:?}");
            let config = configured(&source).expect("valid path");
            assert_eq!(config.string(SETTING), Some(path));
        }
    }

    #[test]
    fn invalid_values_refuse_instead_of_falling_back() {
        for value in [
            "false",
            "42",
            "{}",
            "function() end",
            "''",
            "'../parent'",
            "'a/../b'",
            "'nul\\0path'",
        ] {
            let source = format!("magi.workflow_workspace = {value}");
            let why = configured(&source).expect_err(value).to_string();
            assert!(why.contains("magi.workflow_workspace"), "{why}");
        }
        for path in ["x".repeat(4096), "a/".repeat(256)] {
            let source = format!("magi.workflow_workspace = {path:?}");
            assert!(configured(&source).is_err());
        }
    }

    #[test]
    fn final_assignment_overrides_previously_harvested_value() {
        for (last, succeeds) in [("nil", true), ("function() end", false)] {
            let mut engine = Engine::new();
            engine
                .run("magi.workflow_workspace = '/old'", "init.lua")
                .expect("initial");
            engine.harvest();
            engine
                .run(&format!("magi.workflow_workspace = {last}"), ".magi.lua")
                .expect("override");
            let result = harvest(&mut engine, Path::new("/launch"));
            if succeeds {
                assert!(result.expect("unset").get(SETTING).is_none());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[test]
    fn catalog_workspace_does_not_change_tool_context() {
        let loaded = |source: &str| super::super::Loaded {
            config: configured(source).expect("config"),
            tools: Vec::new(),
            clients: Vec::new(),
        };
        let before = super::super::catalog(&loaded(""), Vec::new());
        let after =
            super::super::catalog(&loaded("magi.workflow_workspace = '/selected'"), Vec::new());
        assert_eq!(before.workflow_workspace, None);
        assert_eq!(
            after.workflow_workspace.as_deref(),
            Some(Path::new("/selected"))
        );
        assert_eq!(before.cwd, after.cwd);
        assert_eq!(before.grants, after.grants);
        assert_eq!(before.confine, after.confine);
        assert_eq!(before.isolate, after.isolate);
        assert_eq!(before.environ, after.environ);
    }
}
