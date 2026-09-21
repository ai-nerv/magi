//! Giving a tool rows and letting it fill them: magi says how much space, forwards what the person
//! does, and blits back what comes out. A surface is a renderer, never an authority.

use magi_proto::tooling::Surface;
use magi_proto::wondering::{Answered, Wonder};

pub const CONFIGURE: &str = "MAGI_TOOLS_CONFIGURE";
pub const CONFIGURE_WAS: &str = "CASPER_CONFIGURE";
pub const JAIL: &str = "CASPER_JAIL";

/// Coordinator-owned settings for one tool invocation and its surfaces.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub configure: String,
    pub jail: Option<String>,
    pub cwd: Option<std::path::PathBuf>,
}

impl Context {
    /// Apply settings without inheriting an ambient jail selector.
    pub fn apply(&self, command: &mut std::process::Command) {
        command
            .env(CONFIGURE, &self.configure)
            .env(CONFIGURE_WAS, &self.configure)
            .env(JAIL, self.jail.as_deref().unwrap_or(""));
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
    }
}

pub trait Holds: Send + Sync {
    /// Reserve the rows, run the surface, and block until it finishes. `None` is not a refusal.
    fn hold(
        &self,
        tool: &str,
        surface: &Surface,
        args: &serde_json::Value,
        context: &Context,
    ) -> Option<String>;
}

/// Something that can answer what a surface asks about the session, blocking like everything else.
pub trait Answers: Send + Sync {
    /// Say what `wonder` asks for, or why not. Never silence — see [`Answered::Refused`].
    fn answer(&self, wonder: Wonder, args: &serde_json::Value) -> Answered;
}

/// An answerer that knows nothing and refuses rather than inventing.
pub struct Incurious;

impl Answers for Incurious {
    fn answer(&self, wonder: Wonder, _args: &serde_json::Value) -> Answered {
        Answered::Refused {
            because: format!("nothing here can answer `{}`", wonder.verb()),
        }
    }
}

/// A holder with no screen behind it, which gives nothing. What `magi -p` uses.
pub struct Screenless;

impl Holds for Screenless {
    fn hold(
        &self,
        _tool: &str,
        _surface: &Surface,
        _args: &serde_json::Value,
        _context: &Context,
    ) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_settings_replace_ambient_selectors() {
        let mut command = std::process::Command::new("unused");
        command.env(JAIL, "stale");
        let context = Context {
            configure: "configured".to_owned(),
            cwd: Some("/workspace".into()),
            jail: None,
        };
        context.apply(&mut command);
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env[std::ffi::OsStr::new(JAIL)],
            Some(std::ffi::OsStr::new(""))
        );
        assert_eq!(
            env[std::ffi::OsStr::new(CONFIGURE)],
            Some(std::ffi::OsStr::new("configured"))
        );
        assert_eq!(
            command.get_current_dir(),
            Some(std::path::Path::new("/workspace"))
        );
    }

    #[test]
    fn no_screen_fills_no_rows_rather_than_pretending_to() {
        let surface = Surface {
            rows: 8,
            about: "the dinosaur game".to_owned(),
            tick: Some(60),
            place: magi_proto::tooling::Place::Prompt,
            tenant: None,
        };
        assert_eq!(
            Screenless.hold(
                "dino",
                &surface,
                &serde_json::Value::Null,
                &Context::default()
            ),
            None
        );
    }
}
