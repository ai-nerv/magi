//! The one builtin that reaches the harness: `spawn`, which starts a child session.
//!
//! magi runs no tools of its own. Reading, writing, editing and running commands are the tools
//! program's job — casper's — reached over the spawn link per call; see `ROLES.md`. `spawn` is not
//! a tool in that sense: it starts another *magi*, which is the harness's own coordination of its
//! agent tree, so it lives here rather than in casper.

use crate::{Cancel, Ops, Output, Tool};
use serde_json::{Value, json};

/// Register the one builtin that reaches the harness: `spawn`, which starts a child session.
/// `environ` is what a child process is told (`MAGI_MELCHIOR_*`, `MAGI_SESSION_PID`); without it a
/// root could not name itself to melchior.
pub fn install_spawn(
    registry: &mut crate::Registry,
    environ: &std::collections::BTreeMap<String, String>,
) {
    registry.register(Box::new(Spawn {
        environ: environ.clone(),
        // Whether this session may start children at all: a parent that spawned it without leave
        // set [`NO_SPAWN`] in its environment, and the flag can only be taken away going down.
        may_spawn: std::env::var(NO_SPAWN).is_err(),
    }));
}

/// Set on a child started without leave to spawn its own; the child's `spawn` refuses while it is
/// there. Inherited, and only ever added going down the tree, so `delegate` narrows like a grant.
pub const NO_SPAWN: &str = "MAGI_NO_SPAWN";

/// Start a child agent of this session. The one builtin that reaches the harness: it runs the very
/// binary this session is — [`std::env::current_exe`], never `magi` off `$PATH`, which could be a
/// stale install — with `fork`, so melchior names the child and caps the tree and magi spawns it.
pub struct Spawn {
    /// What the child is told: the `MAGI_MELCHIOR_*` names and `MAGI_SESSION_PID`, so it can name
    /// itself to melchior and watch the session it belongs to.
    environ: std::collections::BTreeMap<String, String>,
    /// Whether this session was given leave to start children — see [`NO_SPAWN`].
    may_spawn: bool,
}

impl Tool for Spawn {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Start a child agent of this session in the same project. `role` is one word for what it \
         is for; `prompt` is what it should get on with, omitted for one that waits. Returns the \
         child's id; reach it afterwards with the `agent` tool. The tree has a depth and a breadth \
         limit, and starting one past either is refused."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "role": { "type": "string", "description": "One word for what the child is for." },
                "prompt": { "type": "string", "description": "What it should get on with." },
                "delegate": {
                    "type": "boolean",
                    "description": "Whether the child may start children of its own. Default true.",
                }
            }
        })
    }

    fn run(&self, arguments: &Value, ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        // A session started without leave to spawn may not, however it was granted `run`.
        if !self.may_spawn {
            return Output::error(
                "this agent was started without leave to start its own children".to_owned(),
            );
        }
        let Ok(exe) = std::env::current_exe() else {
            return Output::error(
                "magi cannot find its own binary to start a child with".to_owned(),
            );
        };
        let role = arguments["role"].as_str();
        let prompt = arguments["prompt"].as_str();
        // A child started with `delegate: false` is one that may not spawn in turn.
        let delegate = arguments["delegate"].as_bool().unwrap_or(true);
        let mut shown = vec!["fork".to_owned()];
        if let Some(role) = role {
            shown.push(format!("--role={role}"));
        }
        if let Some(prompt) = prompt {
            shown.push(prompt.to_owned());
        }
        // Gated as a `run`, in the words the person sees: "run magi fork …" is a decision, and a
        // grant on it lets an agent start children without asking again.
        if let Err(why) = ops.allow(
            "spawn",
            &magi_proto::permit::Action::Run {
                command: format!("{} {}", exe.display(), shown.join(" ")),
                program: exe.display().to_string(),
            },
        ) {
            return Output::error(why);
        }
        let mut command = std::process::Command::new(&exe);
        command.args(&shown).envs(&self.environ);
        if !delegate {
            command.env(NO_SPAWN, "1");
        }
        match command.output() {
            Ok(out) if out.status.success() => Output {
                content: String::from_utf8_lossy(&out.stdout).trim().to_owned(),
                is_error: false,
                shown: None,
            },
            Ok(out) => Output::error(format!(
                "the child could not be started: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(why) => Output::error(format!("magi fork could not be run: {why}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-spawn", name)
    }

    #[test]
    fn magi_has_no_floor_and_install_spawn_adds_only_spawn() {
        // magi registers no tools of its own — the floor comes from casper now. The one builtin is
        // spawn, and it is not there until install_spawn adds it.
        let mut registry = crate::Registry::new();
        assert_eq!(registry.len(), 0, "magi's registry starts empty");
        install_spawn(&mut registry, &std::collections::BTreeMap::new());
        assert_eq!(registry.len(), 1);
        assert!(
            registry.get("spawn").is_some(),
            "install_spawn did not register spawn"
        );
    }

    #[test]
    fn spawn_asks_before_it_starts_a_child_and_starts_none_when_refused() {
        // A denying gate refuses the run before any process is started: the result is an error and
        // no child was forked.
        let dir = scratch("denied");
        let ops = crate::ops::Real::gated(
            dir.to_path_buf(),
            crate::permit::Ledger::new(),
            std::sync::Arc::new(crate::approve::DenyAll),
        );
        let out = Spawn {
            environ: std::collections::BTreeMap::new(),
            may_spawn: true,
        }
        .run(
            &json!({ "prompt": "do a thing" }),
            &ops,
            &crate::Uncancelled,
        );
        assert!(
            out.is_error,
            "a refused spawn must be an error: {}",
            out.content
        );
    }

    #[test]
    fn a_child_without_leave_may_not_spawn() {
        // Started with `delegate: false` upstream, this session cannot start children whatever it
        // was granted — the refusal is before the gate is even consulted.
        let dir = scratch("no-leave");
        let ops = crate::ops::Real::new(dir.to_path_buf());
        let out = Spawn {
            environ: std::collections::BTreeMap::new(),
            may_spawn: false,
        }
        .run(&json!({ "prompt": "x" }), &ops, &crate::Uncancelled);
        assert!(
            out.is_error,
            "a child without leave spawned anyway: {}",
            out.content
        );
        assert!(out.content.contains("without leave"), "{}", out.content);
    }
}
