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
/// root could not name itself to melchior. `kinds` are the roles the configuration describes.
pub fn install_spawn(
    registry: &mut crate::Registry,
    environ: &std::collections::BTreeMap<String, String>,
    kinds: &[Kind],
) {
    // Whether this session may start children at all: a parent that spawned it without leave set
    // [`NO_SPAWN`] in its environment, and the flag can only be taken away going down.
    let may_spawn = std::env::var(NO_SPAWN).is_err();
    registry.register(Box::new(Spawn::new(
        environ.clone(),
        may_spawn,
        kinds.to_vec(),
    )));
}

/// A role the configuration describes, as `spawn` offers it: what it is called, what it is for, and
/// whether a child in it may start children of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kind {
    pub name: String,
    pub description: String,
    pub delegate: bool,
}

/// The most of a role's description melchior will file with a child's name.
const DESCRIBED: usize = 280;

/// Set on a child started without leave to spawn its own; the child's `spawn` refuses while it is
/// there. Inherited, and only ever added going down the tree, so `delegate` narrows like a grant.
pub const NO_SPAWN: &str = "MAGI_NO_SPAWN";

/// The grants of the session that started a child, as JSON: the child may do what its parent may,
/// no wider. Read once, when the child starts.
pub const GRANTS: &str = "MAGI_GRANTS";

/// Start a child agent of this session. The one builtin that reaches the harness: it runs the very
/// binary this session is — [`std::env::current_exe`], never `magi` off `$PATH`, which could be a
/// stale install — with `fork`, so melchior names the child and caps the tree and magi spawns it.
pub struct Spawn {
    /// What the child is told: the `MAGI_MELCHIOR_*` names and `MAGI_SESSION_PID`, so it can name
    /// itself to melchior and watch the session it belongs to.
    environ: std::collections::BTreeMap<String, String>,
    /// Whether this session was given leave to start children — see [`NO_SPAWN`].
    may_spawn: bool,
    /// The roles a child can be given by name, and the description that lists them for the model.
    kinds: Vec<Kind>,
    described: String,
}

/// What the model is told `spawn` does, before the configured roles are listed.
const DESCRIPTION: &str = "Start a child agent in this project to do one part of a larger task, \
alongside others. It sees none of your conversation, so `prompt` must be a complete brief. Returns \
the child's id; you are woken when it finishes, and its report arrives in your inbox (`agent` tool). \
The tree has a depth and a breadth limit, and starting one past either is refused.";

impl Spawn {
    #[must_use]
    pub fn new(
        environ: std::collections::BTreeMap<String, String>,
        may_spawn: bool,
        kinds: Vec<Kind>,
    ) -> Self {
        let mut described = DESCRIPTION.to_owned();
        if !kinds.is_empty() {
            described.push_str(
                "\n\nRoles a child can be given as `role`, each with instructions of its own:",
            );
            for kind in &kinds {
                described.push_str(&format!("\n- `{}`: {}", kind.name, kind.description));
            }
        }
        Self {
            environ,
            may_spawn,
            kinds,
            described,
        }
    }
}

/// A child's task with a closing line to report back. The coordinator's own reaction is to read
/// its inbox when a child finishes, so the child is told to `send` its findings there. Named to the
/// parent when its id is known, and to "the one that started you" (which `whoami` gives) otherwise.
fn report_back(prompt: &str, parent: Option<&str>) -> String {
    let tail = match parent {
        Some(who) => format!(
            "When you have finished, use the agent tool to send your findings to `{who}` \
             (verb `send`) so the session that started you has your report."
        ),
        None => "When you have finished, use the agent tool to send your findings to the session \
                 that started you — `whoami` names it — so it has your report."
            .to_owned(),
    };
    format!("{prompt}\n\n{tail}")
}

impl Tool for Spawn {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        &self.described
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "role": {
                    "type": "string",
                    "description": "What the child is for: one of the roles listed above, or any \
                                    one word like `backend` or `tests`.",
                },
                "prompt": {
                    "type": "string",
                    "description": "The complete brief: the goal, the files it owns and must stay \
                                    within, the interfaces or contract it must follow, constraints, \
                                    how to check its own work, and what to report back.",
                },
                "delegate": {
                    "type": "boolean",
                    "description": "Whether the child may start children of its own. Default true.",
                }
            },
            "required": ["prompt"],
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
        let kind = role.and_then(|role| self.kinds.iter().find(|kind| kind.name == role));
        // A child started with `delegate: false`, or in a role that may not, may not spawn in turn.
        let delegate = arguments["delegate"].as_bool().unwrap_or(true)
            && kind.is_none_or(|kind| kind.delegate);
        let mut shown = vec!["fork".to_owned()];
        if let Some(role) = role {
            shown.push(format!("--role={role}"));
        }
        // What the `run` grant sees: binary and role, not the free-text `prompt` — whose `(` would
        // trip the chain guard and defeat a standing grant, and which is the child's task anyway.
        let asked = format!("{} {}", exe.display(), shown.join(" "));
        // Filed by melchior with the child's name. Free text, so like the prompt it is kept out of
        // what the grant sees.
        if let Some(kind) = kind.filter(|kind| !kind.description.is_empty()) {
            let about: String = kind.description.chars().take(DESCRIBED).collect();
            shown.push(format!("--role-description={about}"));
        }
        // A coordinator wakes when a child finishes and reads its inbox; a child that never sends
        // leaves it empty. So a task carries a closing line telling the child to report back to the
        // session that started it — this one, named by its own melchior id.
        if let Some(prompt) = prompt {
            shown.push(report_back(
                prompt,
                self.environ.get("MAGI_MELCHIOR_ID").map(String::as_str),
            ));
        }
        if let Err(why) = ops.allow(
            "spawn",
            &magi_proto::permit::Action::Run {
                command: asked,
                program: exe.display().to_string(),
            },
        ) {
            return Output::error(why);
        }
        let mut command = std::process::Command::new(&exe);
        command.args(&shown).envs(&self.environ);
        // The child inherits what this session may do: nobody is attached to a headless child to
        // ask, so without this it is refused what its parent would be allowed.
        if let Ok(held) = serde_json::to_string(&ops.held()) {
            command.env(GRANTS, held);
        }
        if !delegate {
            command.env(NO_SPAWN, "1");
        }
        match command.output() {
            Ok(out) if out.status.success() => Output {
                content: String::from_utf8_lossy(&out.stdout).trim().to_owned(),
                ..Output::default()
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
        install_spawn(&mut registry, &std::collections::BTreeMap::new(), &[]);
        assert_eq!(registry.len(), 1);
        assert!(
            registry.get("spawn").is_some(),
            "install_spawn did not register spawn"
        );
    }

    #[test]
    fn a_child_cannot_be_started_without_a_brief() {
        // A child with no prompt comes up with nothing to do and nothing to report, so the schema
        // refuses the call before the tool runs.
        let spawn = Spawn::new(std::collections::BTreeMap::new(), true, Vec::new());
        let refused = crate::schema::check(&json!({ "role": "backend" }), &spawn.parameters());
        assert!(refused.is_err(), "a spawn with no prompt was accepted");
        let taken = crate::schema::check(&json!({ "prompt": "build it" }), &spawn.parameters());
        assert!(
            taken.is_ok(),
            "a spawn with a prompt was refused: {taken:?}"
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
        let out = Spawn::new(std::collections::BTreeMap::new(), true, Vec::new()).run(
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
    fn a_standing_run_grant_covers_a_spawn_whose_prompt_has_metacharacters() {
        // The gate is asked about the binary and role, not the child's prompt, so a `(` in the
        // prompt does not trip the chain guard and defeat a `run` grant that should stop the asking.
        use magi_proto::permit::{Decision, Grant, Scope};
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Records(std::sync::Arc<AtomicBool>);
        impl crate::approve::Approver for Records {
            fn ask(&self, _tool: &str, _action: &magi_proto::permit::Action) -> Decision {
                self.0.store(true, Ordering::SeqCst);
                Decision::Deny
            }
        }

        let exe = std::env::current_exe().expect("a test binary path");
        let mut ledger = crate::permit::Ledger::new();
        ledger.take_on(vec![Grant {
            verb: "run".to_owned(),
            scope: Scope::Program {
                program: exe.display().to_string(),
            },
        }]);
        let asked = std::sync::Arc::new(AtomicBool::new(false));
        let dir = scratch("standing-grant");
        let ops = crate::ops::Real::gated(
            dir.to_path_buf(),
            ledger,
            std::sync::Arc::new(Records(std::sync::Arc::clone(&asked))),
        );
        Spawn::new(std::collections::BTreeMap::new(), true, Vec::new()).run(
            &json!({ "role": "scanner", "prompt": "look at the magi crate (the nerv repo)" }),
            &ops,
            &crate::Uncancelled,
        );
        assert!(
            !asked.load(Ordering::SeqCst),
            "a standing run grant should have covered the spawn without asking"
        );
    }

    #[test]
    fn a_task_tells_the_child_to_report_back_to_the_session_that_started_it() {
        let told = report_back("scan the magi crate", Some("alpha-mu"));
        assert!(
            told.starts_with("scan the magi crate"),
            "the task is kept: {told}"
        );
        assert!(told.contains("send"), "no reporting instruction: {told}");
        assert!(
            told.contains("`alpha-mu`"),
            "the parent is not named: {told}"
        );
        // Without a known parent it still says to report, by way of `whoami`.
        let rootless = report_back("do a thing", None);
        assert!(
            rootless.contains("whoami"),
            "no fallback recipient: {rootless}"
        );
    }

    #[test]
    fn a_child_without_leave_may_not_spawn() {
        // Started with `delegate: false` upstream, this session cannot start children whatever it
        // was granted — the refusal is before the gate is even consulted.
        let dir = scratch("no-leave");
        let ops = crate::ops::Real::new(dir.to_path_buf());
        let out = Spawn::new(std::collections::BTreeMap::new(), false, Vec::new()).run(
            &json!({ "prompt": "x" }),
            &ops,
            &crate::Uncancelled,
        );
        assert!(
            out.is_error,
            "a child without leave spawned anyway: {}",
            out.content
        );
        assert!(out.content.contains("without leave"), "{}", out.content);
    }

    #[test]
    fn the_configured_roles_are_offered_by_name_and_what_they_are_for() {
        let reviewer = Kind {
            name: "reviewer".to_owned(),
            description: "reads a change".to_owned(),
            delegate: false,
        };
        let offered = Spawn::new(std::collections::BTreeMap::new(), true, vec![reviewer]);
        assert!(
            offered.description().contains("`reviewer`: reads a change"),
            "{}",
            offered.description()
        );
        let bare = Spawn::new(std::collections::BTreeMap::new(), true, Vec::new());
        assert!(!bare.description().contains("Roles"), "no roles, no list");
    }
}
