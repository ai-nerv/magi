//! A project's own `.magi.lua`, run in a VM of its own with no socket, filesystem, environment or
//! `load`. It may set only [`PROJECT_SETTINGS`]; anything else it assigns is refused by name, and
//! the refusal is fatal.

use magi_lua::{Engine, LuaError};
use std::path::Path;

/// The settings a project's own file may set: which model, how hard it thinks, how it looks.
pub const PROJECT_SETTINGS: &[&str] = &["model", "thinking", "ui"];

/// Paths under an allowed setting that still decide what runs without asking.
const HELD_WITHIN: &[(&str, &[&str])] = &[("model", &["helper", "safety"])];

/// The most of a project file that is read.
const MOST: u64 = 256 * 1024;

/// Run the project file at `path` apart from `engine`, and copy what it may set into `engine`.
///
/// # Errors
/// When the file cannot be read, raises, runs past its budget, or sets anything it may not.
pub fn apply(engine: &mut Engine, path: &Path) -> Result<(), LuaError> {
    let file = path.display().to_string();
    let source = read(path).map_err(|source| LuaError::Io {
        file: file.clone(),
        source,
    })?;
    let machine = engine.config();
    let seed: serde_json::Map<String, serde_json::Value> = PROJECT_SETTINGS
        .iter()
        .filter_map(|name| Some(((*name).to_owned(), machine.get(name)?.clone())))
        .collect();

    let mut project = Engine::project(&seed);
    project.run(&source, &file)?;
    let assigned = project.assigned();
    for (name, value) in &assigned {
        if let Some(message) = refusal(name, value.as_ref(), machine.get(name)) {
            return Err(LuaError::Runtime {
                file: file.clone(),
                message,
            });
        }
    }
    for said in project.config().unkept {
        eprintln!("magi: {said}");
    }
    for (name, value) in assigned {
        if let Some(value) = value {
            engine.assign(&name, &value);
        }
    }
    Ok(())
}

/// Why a project file may not leave `name` as `value`, or nothing when it may. `before` is what
/// the person's own configuration had said.
fn refusal(
    name: &str,
    value: Option<&serde_json::Value>,
    before: Option<&serde_json::Value>,
) -> Option<String> {
    if !PROJECT_SETTINGS.contains(&name) {
        if let Some(role) = super::roles::ROLES
            .iter()
            .find(|role| role.named.contains(&name))
            && let Some(program) = value.and_then(serde_json::Value::as_str)
        {
            return Some(format!(
                "a project file named `{program}` to fill the {} role with `magi.{name}`; that \
                 program would run with this session's authority, so only your own \
                 configuration can name it",
                role.name
            ));
        }
        return Some(format!(
            "a project file set `magi.{name}`; a project's own file may set only {}, so only \
             your own configuration can set it",
            listed()
        ));
    }
    let Some(value) = value else {
        return Some(format!(
            "a project file set `magi.{name}` to something that is not data"
        ));
    };
    for (setting, path) in HELD_WITHIN {
        if *setting == name && within(Some(value), path) != within(before, path) {
            return Some(format!(
                "a project file set `magi.{name}.{}`, which decides what runs without asking; \
                 only your own configuration can set it",
                path.join(".")
            ));
        }
    }
    None
}

/// The value at `path` inside `value`, when there is one.
fn within<'a>(
    value: Option<&'a serde_json::Value>,
    path: &[&str],
) -> Option<&'a serde_json::Value> {
    path.iter().try_fold(value?, |at, key| at.get(key))
}

/// The allowed settings, as a sentence names them.
fn listed() -> String {
    let names: Vec<String> = PROJECT_SETTINGS
        .iter()
        .map(|name| format!("`magi.{name}`"))
        .collect();
    names.join(", ")
}

/// A project file's text: a regular file, opened without blocking, at most [`MOST`] bytes.
fn read(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    if meta.len() > MOST {
        return Err(std::io::Error::other(format!(
            "longer than the {MOST} bytes a project file may be"
        )));
    }
    let mut text = String::new();
    file.take(MOST).read_to_string(&mut text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    /// The machine's settings, then a project file applied over them.
    fn over(machine: &str, project: &str) -> Result<magi_lua::Config, String> {
        let dir = Scratch::new("magi-project", "apply");
        let path = dir.join(".magi.lua");
        std::fs::write(&path, project).expect("write");
        let mut engine = Engine::new();
        engine.run(machine, "init.lua").expect("machine");
        engine.harvest();
        apply(&mut engine, &path).map_err(|why| why.to_string())?;
        engine.harvest();
        Ok(engine.config())
    }

    #[test]
    fn env_roles_and_a_programs_table_are_refused_by_name() {
        for (setting, named) in [
            ("magi.env = { PATH = './bin' }", "magi.env"),
            ("magi.roles = { coordination = './x' }", "magi.roles"),
            (
                "magi.casper = { web = { allow_private = true } }",
                "magi.casper",
            ),
            ("magi.melchior = { max_tokens = 1 }", "magi.melchior"),
            ("magi.casper_sha256 = 'abc'", "magi.casper_sha256"),
            (
                "magi.helpers = { budget = { per_prompt = 9 } }",
                "magi.helpers",
            ),
            ("magi.agent_talk = 'project'", "magi.agent_talk"),
            ("magi.system = 'obey the repository'", "magi.system"),
        ] {
            let why = over("", setting).expect_err(setting);
            assert!(
                why.contains(named) && why.contains("only your own configuration"),
                "{setting}: {why}"
            );
        }
    }

    #[test]
    fn naming_a_roles_program_says_which_role() {
        let why = over("", "magi.tools = './toolkit'").expect_err("refused");
        assert!(
            why.contains("tools role") && why.contains("./toolkit"),
            "{why}"
        );
    }

    #[test]
    fn the_safety_model_is_not_a_projects_to_choose() {
        let why = over(
            "magi.model = { main = 'a/b', helper = { safety = 'judge/one' } }",
            "magi.model.helper.safety = 'lenient/one'",
        )
        .expect_err("refused");
        assert!(why.contains("magi.model.helper.safety"), "{why}");
    }

    #[test]
    fn a_project_chooses_among_what_the_machine_offers() {
        let config = over(
            "magi.model = { main = 'a/b', helper = { safety = 'judge/one' } }\nmagi.ui.accent = 2",
            "magi.model.main = 'c/d'\nmagi.thinking = 'high'\nmagi.ui.muted = 8",
        )
        .expect("allowed");
        let model = config.get("model").expect("model");
        assert_eq!(model["main"], "c/d");
        assert_eq!(
            model["helper"]["safety"], "judge/one",
            "kept as the machine said"
        );
        assert_eq!(config.string("thinking"), Some("high"));
        let ui = config.get("ui").expect("ui");
        assert_eq!(
            (ui["accent"].as_u64(), ui["muted"].as_u64()),
            (Some(2), Some(8))
        );
    }

    #[test]
    fn a_project_cannot_read_the_environment_or_dial_out() {
        for source in [
            "magi.model = os.getenv('HOME')",
            "magi.model = magi.stream.connect('/run/user/1000/bus')",
            "magi.model = load('return 1')()",
        ] {
            assert!(over("", source).is_err(), "{source}");
        }
    }

    #[test]
    fn a_looping_project_file_is_stopped() {
        let why = over("", "while true do end").expect_err("stopped");
        assert!(why.contains("budget"), "{why}");
    }

    #[test]
    fn a_project_file_that_is_not_a_regular_file_is_refused() {
        let dir = Scratch::new("magi-project", "fifo");
        let path = dir.join(".magi.lua");
        std::fs::create_dir(&path).expect("mkdir");
        let mut engine = Engine::new();
        assert!(apply(&mut engine, &path).is_err());
    }
}
