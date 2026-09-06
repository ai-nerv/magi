//! A tool that is an ordinary program.
//!
//! The third transport, and the one that makes the rule operable: the core ships `read`, `write`,
//! `edit` and the shell peer, and **everything else is declared in Lua config**. Until this, a
//! config could not declare a tool that runs a program. A Lua tool cannot spawn — the sandbox
//! removes `os.execute` so that spawning cannot happen outside the [`Ops`] seam — and the process
//! transport wants a peer speaking magi's framed protocol, so `command = "rg"` was a broken pipe
//! rather than a search.
//!
//! So `grep` was a shell string the model composed, which costs a round trip whenever the quoting
//! is wrong and returns however much the tree happens to hold.
//!
//! **There is no shell here.** The program is executed directly with an argument vector built from
//! the call. A value containing `;` or `$(…)` is one argument, verbatim, and that is what makes
//! this safe to hand to a config file. See [`render`].

use crate::{Cancel, Ops, Output, Sending, Tool};
use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, Instant};

/// Bytes read from a child before it is cut off.
///
/// Ahead of [`crate::bound`], which caps what the model is shown. This one is about memory: a
/// program that never stops printing must not be read into the process in full first.
const MAX_READ: usize = 4 * 1024 * 1024;

/// Seconds a program is allowed when its declaration does not say.
const DEFAULT_TIMEOUT: u64 = 120;

/// How often a running child is checked while waiting for it.
const POLL: Duration = Duration::from_millis(20);

/// A tool that runs one program and returns what it printed.
pub struct CommandTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    timeout: Duration,
}

impl CommandTool {
    /// Declare one.
    #[must_use]
    pub fn new(
        name: &str,
        description: &str,
        parameters: serde_json::Value,
        program: &str,
        args: Vec<String>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters,
            program: program.to_owned(),
            args,
            env: BTreeMap::new(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT),
        }
    }

    /// Environment beside what every process magi starts already gets.
    #[must_use]
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// How long it may run.
    #[must_use]
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout = Duration::from_secs(seconds.max(1));
        self
    }

    /// The placeholders this declaration uses, for checking against the schema at install time.
    #[must_use]
    pub fn placeholders(&self) -> Vec<String> {
        let mut out = Vec::new();
        for arg in &self.args {
            if let Some(name) = whole_placeholder(arg) {
                out.push(name.to_owned());
            } else {
                out.extend(embedded(arg));
            }
        }
        out
    }

    /// What a person is asked before this runs.
    fn action(&self, rendered: &[String]) -> magi_proto::permit::Action {
        let mut shown = vec![self.program.clone()];
        shown.extend(rendered.iter().cloned());
        magi_proto::permit::Action::Run {
            command: shown.join(" "),
            program: self.program.clone(),
        }
    }
}

impl Tool for CommandTool {
    fn composition(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("transport", "command".to_owned()),
            (
                "command",
                std::iter::once(self.program.clone())
                    .chain(self.args.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        ];
        if !self.env.is_empty() {
            out.push((
                "env",
                self.env
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
        out
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let rendered = render(&self.args, arguments);
        // Asked before anything is started. A tool that runs and then asks has already done the
        // thing the question was about.
        if let Err(why) = ops.allow(&self.name, &self.action(&rendered)) {
            return Output::error(why);
        }
        match spawn(&self.program, &rendered, &self.env, ops, self.timeout) {
            Ok(finished) => finished.into_output(&self.program),
            Err(why) => Output::error(why),
        }
    }

    // Inline, deliberately. Overlap is worth having between peers, which hold a connection and
    // answer one call at a time; a program that runs and exits has a `run` that is already the
    // whole of it, and a second process to manage it would buy nothing.
    fn send(&self, _arguments: &serde_json::Value, _ops: &dyn Ops) -> Sending {
        Sending::Inline
    }
}

/// What a program did.
struct Finished {
    out: String,
    err: String,
    code: Option<i32>,
}

impl Finished {
    /// The answer the model reads.
    ///
    /// **A non-zero exit is not an error.** `rg`, `grep` and `fd` all exit 1 for "nothing
    /// matched", and treating that as a failure makes every unsuccessful search look like a
    /// broken tool. It is only an error when the program said nothing at all — then the exit
    /// status is the only thing there is to report, and stderr is what explains it.
    fn into_output(self, program: &str) -> Output {
        if !self.out.is_empty() {
            return Output {
                content: self.out,
                is_error: false,
                shown: None,
            };
        }
        match self.code {
            Some(0) => Output {
                content: String::new(),
                is_error: false,
                shown: None,
            },
            Some(code) if self.err.is_empty() => Output {
                content: format!("{program} exited {code} with no output"),
                is_error: false,
                shown: None,
            },
            Some(code) => Output::error(format!("{program} exited {code}: {}", self.err.trim())),
            None => Output::error(format!("{program} was killed: {}", self.err.trim())),
        }
    }
}

/// Run the program and collect what it printed.
fn spawn(
    program: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    ops: &dyn Ops,
    timeout: Duration,
) -> Result<Finished, String> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .current_dir(ops.cwd())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::environ::apply(&mut command, env);

    let mut child = command
        .spawn()
        .map_err(|why| format!("{program} could not be run: {why}"))?;

    // Drained on threads because a program that fills one pipe while nothing reads the other
    // blocks forever, and which pipe it fills is not ours to predict.
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);

    let deadline = Instant::now() + timeout;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{program} was still running after {}s",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(POLL);
            }
            Err(why) => return Err(format!("{program} could not be waited for: {why}")),
        }
    };

    Ok(Finished {
        out: out.map(collect).unwrap_or_default(),
        err: err.map(collect).unwrap_or_default(),
        code,
    })
}

/// Read a pipe to its end on a thread of its own, up to [`MAX_READ`].
fn drain<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 8192];
        while buffer.len() < MAX_READ {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            }
        }
        buffer.truncate(MAX_READ);
        String::from_utf8_lossy(&buffer).into_owned()
    })
}

/// Collect what a drain thread read, or nothing if it panicked.
fn collect(handle: std::thread::JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

/// Build the argument vector from the templates and the call.
///
/// A template is a literal, except that `{name}` takes the value of the call's `name` argument.
/// Three rules, and each is here because the alternative is worse:
///
/// - A template naming an argument the call did not send is dropped **whole**, flag and all.
///   `--max-count={limit}` with no `limit` must not become `--max-count=`: most programs read a
///   flag with an empty value as malformed rather than as unset, and the one that does not reads
///   it as a value of "". This holds however the placeholder is written — the rule is about the
///   argument being absent, not about where in the template it sits.
/// - A template that is exactly `{name}` and whose argument is an array becomes one argument per
///   element, so a list of paths is a list of paths.
/// - Values are never interpreted. This is an argument vector, not a command line: nothing here
///   splits on whitespace, expands a glob, or reads a `;`.
#[must_use]
pub fn render(templates: &[String], arguments: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    for template in templates {
        if let Some(name) = whole_placeholder(template) {
            match arguments.get(name) {
                None | Some(serde_json::Value::Null) => {}
                Some(serde_json::Value::Array(items)) => {
                    out.extend(items.iter().map(scalar));
                }
                Some(value) => out.push(scalar(value)),
            }
            continue;
        }
        if embedded(template)
            .iter()
            .any(|name| !given(arguments, name))
        {
            continue;
        }
        out.push(substitute(template, arguments));
    }
    out
}

/// Whether the call actually carries a value for `name`.
fn given(arguments: &serde_json::Value, name: &str) -> bool {
    !matches!(arguments.get(name), None | Some(serde_json::Value::Null))
}

/// The name in a template that is nothing but one placeholder.
fn whole_placeholder(template: &str) -> Option<&str> {
    let inner = template.strip_prefix('{')?.strip_suffix('}')?;
    (!inner.is_empty() && !inner.contains(['{', '}'])).then_some(inner)
}

/// Every placeholder named inside a template that is not only a placeholder.
fn embedded(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        let name = &after[..close];
        if !name.is_empty() && !name.contains('{') {
            out.push(name.to_owned());
        }
        rest = &after[close + 1..];
    }
    out
}

/// Replace every `{name}` inside a template with its value.
fn substitute(template: &str, arguments: &serde_json::Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let name = &after[..close];
        if let Some(value) = arguments.get(name) {
            out.push_str(&scalar(value));
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// One argument's value, as the program will see it.
///
/// A string is itself rather than its JSON spelling: a path argument must arrive as `src/main.rs`
/// and not as `"src/main.rs"`, quotes included.
fn scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Building the argument vector from a call.
#[cfg(test)]
mod render_tests {
    use super::*;
    use serde_json::json;

    fn rendered(templates: &[&str], arguments: &serde_json::Value) -> Vec<String> {
        let templates: Vec<String> = templates.iter().map(|t| (*t).to_owned()).collect();
        render(&templates, arguments)
    }

    #[test]
    fn a_literal_argument_is_passed_through() {
        assert_eq!(rendered(&["--json", "-n"], &json!({})), ["--json", "-n"]);
    }

    #[test]
    fn a_placeholder_takes_the_value_from_the_call() {
        assert_eq!(
            rendered(&["{pattern}"], &json!({ "pattern": "TODO" })),
            ["TODO"]
        );
    }

    #[test]
    fn a_placeholder_inside_a_flag_is_substituted() {
        assert_eq!(
            rendered(&["--max-count={limit}"], &json!({ "limit": 20 })),
            ["--max-count=20"]
        );
    }

    #[test]
    fn an_absent_bare_placeholder_is_dropped() {
        assert!(rendered(&["{limit}"], &json!({})).is_empty());
    }

    #[test]
    fn an_absent_argument_takes_its_flag_with_it() {
        // The one that got `rg` a `--glob=` with nothing after it. `--max-count=` is read by
        // most programs as malformed rather than as unset, and by the rest as a value of "".
        assert!(rendered(&["--max-count={limit}"], &json!({})).is_empty());
        assert!(rendered(&["--glob={glob}"], &json!({ "pattern": "x" })).is_empty());
    }

    #[test]
    fn a_null_argument_counts_as_absent() {
        // A model that sends `{"glob": null}` has said nothing, not said empty.
        assert!(rendered(&["--glob={glob}"], &json!({ "glob": null })).is_empty());
    }

    #[test]
    fn a_template_naming_two_arguments_needs_both() {
        assert!(rendered(&["{a}-{b}"], &json!({ "a": "x" })).is_empty());
        assert_eq!(
            rendered(&["{a}-{b}"], &json!({ "a": "x", "b": "y" })),
            ["x-y"]
        );
    }

    #[test]
    fn an_array_argument_expands_to_one_argument_each() {
        assert_eq!(
            rendered(&["{paths}"], &json!({ "paths": ["a.rs", "b.rs"] })),
            ["a.rs", "b.rs"]
        );
    }

    #[test]
    fn a_string_arrives_without_its_json_quotes() {
        // A path argument must be `src/main.rs`, not `"src/main.rs"` with the quotes in it.
        assert_eq!(
            rendered(&["{path}"], &json!({ "path": "src/main.rs" })),
            ["src/main.rs"]
        );
    }

    #[test]
    fn a_value_is_never_shell_interpreted() {
        // The security property this transport rests on: an argument vector is not a command
        // line. Nothing here splits on whitespace, expands a glob, or reads a `;`.
        let hostile = "a; rm -rf /; echo $(whoami) `id` && :";
        assert_eq!(
            rendered(&["{pattern}"], &json!({ "pattern": hostile })),
            [hostile]
        );
    }

    #[test]
    fn a_placeholder_the_schema_does_not_declare_is_reported() {
        let tool = CommandTool::new(
            "grep",
            "",
            json!({ "type": "object", "properties": { "pattern": { "type": "string" } } }),
            "rg",
            vec!["{pattern}".to_owned(), "--max-count={limit}".to_owned()],
        );
        assert_eq!(tool.placeholders(), ["pattern", "limit"]);
    }
}

/// Running the program.
#[cfg(test)]
mod running_tests {
    use super::*;
    use crate::cancel::Uncancelled;
    use crate::ops::Real;
    use magi_model::scratch::Scratch;
    use serde_json::json;

    /// An `Ops` that refuses every call, to prove the gate is asked before anything runs.
    struct Refusing(std::path::PathBuf);

    impl Ops for Refusing {
        fn cwd(&self) -> std::path::PathBuf {
            self.0.clone()
        }
        fn read(&self, _path: &std::path::Path) -> Result<String, String> {
            Err("no".to_owned())
        }
        fn write(&self, _path: &std::path::Path, _contents: &str) -> Result<(), String> {
            Err("no".to_owned())
        }
        fn shell(&self, _command: &str) -> Result<crate::ops::Shell, String> {
            Err("no".to_owned())
        }
        fn allow(&self, _tool: &str, _action: &magi_proto::permit::Action) -> Result<(), String> {
            Err("the person said no".to_owned())
        }
    }

    fn echo(args: Vec<&str>) -> CommandTool {
        CommandTool::new(
            "say",
            "prints its argument",
            json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
            "echo",
            args.into_iter().map(str::to_owned).collect(),
        )
    }

    #[test]
    fn a_program_runs_and_returns_what_it_printed() {
        let dir = std::env::temp_dir();
        let ops = Real::new(dir);
        let out = echo(vec!["{text}"]).run(&json!({ "text": "hello" }), &ops, &Uncancelled);
        assert!(!out.is_error, "{out:?}");
        assert_eq!(out.content.trim(), "hello");
    }

    #[test]
    fn a_hostile_value_reaches_the_program_as_one_argument() {
        // The other half of `a_value_is_never_shell_interpreted`: not just that the vector is
        // built literally, but that nothing between here and the program re-splits it.
        let ops = Real::new(std::env::temp_dir());
        let out = echo(vec!["{text}"]).run(&json!({ "text": "a; echo b" }), &ops, &Uncancelled);
        assert_eq!(out.content.trim(), "a; echo b", "a shell got involved");
    }

    #[test]
    fn a_refused_call_never_runs_the_program() {
        let dir = Scratch::new("magi-refused", "one");
        let marker = dir.join("ran");
        let tool = CommandTool::new(
            "touching",
            "",
            json!({ "type": "object" }),
            "touch",
            vec![marker.display().to_string()],
        );
        let out = tool.run(&json!({}), &Refusing(dir.to_path_buf()), &Uncancelled);
        assert!(out.is_error, "{out:?}");
        assert!(
            !marker.exists(),
            "the gate refused and the program ran anyway"
        );
    }

    #[test]
    fn nothing_matched_is_not_a_failure() {
        // `rg`, `grep` and `fd` all exit 1 for "nothing matched". Reporting that as a broken
        // tool makes every unsuccessful search look like one.
        let ops = Real::new(std::env::temp_dir());
        let tool = CommandTool::new("nope", "", json!({ "type": "object" }), "false", vec![]);
        let out = tool.run(&json!({}), &ops, &Uncancelled);
        assert!(!out.is_error, "{out:?}");
    }

    #[test]
    fn a_program_that_is_not_there_is_reported_not_panicked() {
        let ops = Real::new(std::env::temp_dir());
        let tool = CommandTool::new(
            "missing",
            "",
            json!({ "type": "object" }),
            "magi-no-such-program-anywhere",
            vec![],
        );
        let out = tool.run(&json!({}), &ops, &Uncancelled);
        assert!(out.is_error);
        assert!(
            out.content.contains("magi-no-such-program-anywhere"),
            "{}",
            out.content
        );
    }

    #[test]
    fn a_program_that_will_not_stop_is_killed_and_said_so() {
        let ops = Real::new(std::env::temp_dir());
        let tool = CommandTool::new(
            "waiting",
            "",
            json!({ "type": "object" }),
            "sleep",
            vec!["30".to_owned()],
        )
        .with_timeout(1);
        let out = tool.run(&json!({}), &ops, &Uncancelled);
        assert!(out.is_error, "{out:?}");
        assert!(out.content.contains("still running"), "{}", out.content);
    }
}
