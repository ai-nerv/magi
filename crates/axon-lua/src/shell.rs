//! Running a command from a Lua tool.
//!
//! The sandbox removes `os.execute` and `io.popen`, and keeps removing them. The objection those
//! answer is not "a config must never run anything" — it is that spawning outside the [`Ops`] seam
//! happens with nothing checking the path, nothing asking the person, and nothing bounding what
//! comes back. `axon.shell` runs *through* the seam: the same [`Ops::allow`] the shell peer is
//! gated by, the same permission scopes, the same refusals.
//!
//! ```lua
//! local out, err = axon.shell("rg --json -e " .. pattern)
//! ```
//!
//! **What this is for, and what it is not.** A tool that runs one fixed program with arguments
//! from the call is a `command` transport — declared, no code. This is for the rest: choosing
//! `rg` when it is there and `grep` when it is not, running two things and merging them, reading
//! an answer and reshaping it before the model sees it.
//!
//! **Only inside a tool call.** At config load time there is no call, nobody has been asked
//! anything, and a description that could spawn while it was being *read* would be a worse hole
//! than the one this closes. The slot is empty except while a tool's `run` is on the stack.

use axon_tools::Ops;
use luna::{Callback, CallbackReturn, Context, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The session's `Ops`, lent to the VM for as long as the worker lives.
pub type Lent = Rc<RefCell<Option<Rc<dyn Ops>>>>;

/// Whether a tool's `run` is on the stack right now.
pub type Inside = Rc<Cell<bool>>;

/// What a caller outside a tool is told.
const OUTSIDE: &str = "axon.shell is only available inside a tool's run function";

/// What a caller is told when the daemon never lent an `Ops` — every path except a real session.
const UNAVAILABLE: &str = "axon.shell is not available here";

/// Build the `axon.shell` function.
pub fn callback<'gc>(ctx: Context<'gc>, lent: Lent, inside: Inside) -> Callback<'gc> {
    Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        let command: Value = stack.consume(ctx)?;
        let Value::String(command) = command else {
            stack.replace(ctx, (Value::Nil, "axon.shell(command): a string"));
            return Ok(CallbackReturn::Return);
        };
        let command = String::from_utf8_lossy(command.as_bytes()).into_owned();

        if !inside.get() {
            stack.replace(ctx, (Value::Nil, OUTSIDE));
            return Ok(CallbackReturn::Return);
        }
        let held = lent.borrow();
        let Some(ops) = held.as_ref() else {
            stack.replace(ctx, (Value::Nil, UNAVAILABLE));
            return Ok(CallbackReturn::Return);
        };

        // Asked before it runs, and asked as a *command* with its program named separately, so
        // "any `rg` command" is an answer somebody can actually give.
        let action = axon_tools::permit::Action::Run {
            command: command.clone(),
            program: axon_tools::process::first_word(&command),
        };
        if let Err(why) = ops.allow("shell", &action) {
            stack.replace(ctx, (Value::Nil, why));
            return Ok(CallbackReturn::Return);
        }

        match ops.shell(&command) {
            // A non-zero exit is not a failure. `rg`, `grep` and `fd` all exit 1 for "nothing
            // matched", and the tool is the one that knows whether that mattered.
            Ok(answer) => {
                let out = luna::String::from_slice(&ctx, answer.stdout.as_bytes());
                if answer.ok() {
                    stack.replace(ctx, (out, Value::Nil));
                } else {
                    let err = luna::String::from_slice(&ctx, answer.stderr.as_bytes());
                    stack.replace(ctx, (out, err));
                }
            }
            Err(why) => stack.replace(ctx, (Value::Nil, why)),
        }
        Ok(CallbackReturn::Return)
    })
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use axon_tools::ops::Shell;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// An `Ops` that records what it was asked to run, and whether it allowed it.
    struct Watching {
        allow: bool,
        ran: Mutex<Vec<String>>,
    }

    impl axon_tools::Ops for Watching {
        fn cwd(&self) -> PathBuf {
            std::env::temp_dir()
        }
        fn read(&self, _path: &Path) -> Result<String, String> {
            Err("no".to_owned())
        }
        fn write(&self, _path: &Path, _contents: &str) -> Result<(), String> {
            Err("no".to_owned())
        }
        fn shell(&self, command: &str) -> Result<Shell, String> {
            if let Ok(mut ran) = self.ran.lock() {
                ran.push(command.to_owned());
            }
            Ok(Shell {
                code: Some(0),
                stdout: format!("ran {command}"),
                stderr: String::new(),
            })
        }
        fn allow(&self, _tool: &str, _action: &axon_tools::permit::Action) -> Result<(), String> {
            if self.allow {
                Ok(())
            } else {
                Err("the person said no".to_owned())
            }
        }
    }

    /// Declare a tool whose body calls `axon.shell`, call it, and say what came back.
    fn through(ops: Option<std::rc::Rc<Watching>>, body: &str) -> (String, Option<String>) {
        let mut engine = Engine::new();
        if let Some(ops) = ops.clone() {
            engine.attach_ops(ops);
        }
        engine
            .run(
                &format!(
                    "axon.tool(\"probe\", {{ transport = {{ kind = \"lua\" }},\n\
                       run = function(args) {body} end }})"
                ),
                "probe.lua",
            )
            .expect("the declaration runs");
        engine.harvest();
        let answer = engine
            .call_tool("probe", &serde_json::json!({}))
            .unwrap_or(serde_json::Value::Null);
        let content = answer
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_owned();
        let ran = ops.and_then(|o| o.ran.lock().ok().and_then(|r| r.first().cloned()));
        (content, ran)
    }

    #[test]
    fn a_tool_can_run_a_command() {
        let ops = std::rc::Rc::new(Watching {
            allow: true,
            ran: Mutex::new(Vec::new()),
        });
        let (content, ran) = through(
            Some(ops),
            "local out = axon.shell('echo hi') return { content = out }",
        );
        assert_eq!(ran.as_deref(), Some("echo hi"), "it reached the seam");
        assert_eq!(content, "ran echo hi");
    }

    #[test]
    fn a_refused_command_never_runs() {
        // The whole value of this over `os.execute`: it goes through the gate. If a future
        // change makes the gate optional this becomes `os.execute` with extra steps.
        let ops = std::rc::Rc::new(Watching {
            allow: false,
            ran: Mutex::new(Vec::new()),
        });
        let (content, ran) = through(
            Some(ops),
            "local out, err = axon.shell('rm -rf /') return { content = tostring(err) }",
        );
        assert_eq!(ran, None, "the gate refused and it ran anyway");
        assert!(content.contains("said no"), "{content}");
    }

    #[test]
    fn without_a_seam_it_says_so_rather_than_running() {
        // `axon tools`, a config being read, a test: every path but a real session lends no
        // `Ops`, and a native that quietly did nothing would be worse than one that answers.
        let (content, _) = through(
            None,
            "local out, err = axon.shell('echo hi') return { content = tostring(err) }",
        );
        assert!(content.contains("not available"), "{content}");
    }

    #[test]
    fn a_config_being_read_cannot_spawn() {
        // The boundary that keeps this from being worse than what the sandbox removed: a
        // description that could run something while it was being *read* would spawn before
        // anybody had been asked anything.
        let mut engine = Engine::new();
        engine.attach_ops(std::rc::Rc::new(Watching {
            allow: true,
            ran: Mutex::new(Vec::new()),
        }));
        engine
            .run(
                "local out, err = axon.shell('echo hi')\naxon.answer = tostring(err)",
                "at-load.lua",
            )
            .expect("it runs");
        engine.harvest();
        let said = engine.config().get("answer").cloned().unwrap_or_default();
        assert!(
            said.as_str().unwrap_or_default().contains("inside a tool"),
            "{said:?}"
        );
    }

    #[test]
    fn a_non_string_argument_is_refused_rather_than_raising() {
        let ops = std::rc::Rc::new(Watching {
            allow: true,
            ran: Mutex::new(Vec::new()),
        });
        let (content, ran) = through(
            Some(ops),
            "local out, err = axon.shell(42) return { content = tostring(err) }",
        );
        assert_eq!(ran, None);
        assert!(content.contains("a string"), "{content}");
    }

    #[test]
    fn the_sandbox_still_refuses_the_other_way_in() {
        // `axon.shell` is an addition, not a relaxation. `os.execute` stays gone.
        let mut engine = Engine::new();
        engine
            .run("axon.answer = tostring(os.execute)", "probe.lua")
            .expect("runs");
        engine.harvest();
        assert_eq!(
            engine.config().get("answer").and_then(|v| v.as_str()),
            Some("nil")
        );
    }
}
