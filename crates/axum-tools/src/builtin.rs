//! The three tools that are the floor.
//!
//! Pure filesystem, already behind [`Ops`], and the ones a session cannot do without. Anything
//! that wants isolation, a language of its own, or a life longer than one call is a declared
//! tool in `config/tools/` — which is where `bash` lives.

use crate::{Cancel, Ops, Output, Tool};
use serde_json::{Value, json};
use std::path::Path;

/// Lines of a file shown when no range is asked for.
const PREVIEW_LINES: usize = 2000;

/// Register the floor.
pub fn install(registry: &mut crate::Registry) {
    registry.register(Box::new(Read));
    registry.register(Box::new(Write));
    registry.register(Box::new(Edit));
}

/// A required string argument, or a message saying which one is missing.
fn arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, Output> {
    arguments[name]
        .as_str()
        .ok_or_else(|| Output::error(format!("{name} is required and must be a string")))
}

/// Read a file.
pub struct Read;

impl Tool for Read {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a file from the session's directory. Returns the contents with line numbers, \
         which is what `edit` matches against."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path, relative to the session." },
            },
            "required": ["path"],
        })
    }

    fn run(&self, arguments: &Value, ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let path = match arg(arguments, "path") {
            Ok(path) => path,
            Err(output) => return output,
        };
        match ops.read(Path::new(path)) {
            Err(why) => Output::error(why),
            Ok(contents) => {
                let lines: Vec<&str> = contents.lines().collect();
                let shown = lines.len().min(PREVIEW_LINES);
                // Numbered because `edit` matches on content and a model that can count lines
                // makes better patches than one guessing at them.
                let mut out = String::new();
                for (index, line) in lines[..shown].iter().enumerate() {
                    out.push_str(&format!("{:>6}\t{line}\n", index + 1));
                }
                if lines.len() > shown {
                    out.push_str(&format!(
                        "… {} more lines; ask for them with a shell command if you need them\n",
                        lines.len() - shown
                    ));
                }
                Output::ok(out)
            }
        }
    }
}

/// Write a whole file.
pub struct Write;

impl Tool for Write {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write a file, replacing it entirely and creating parent directories. Prefer `edit` for \
         a change to a file that already exists."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path, relative to the session." },
                "contents": { "type": "string", "description": "The whole new contents." },
            },
            "required": ["path", "contents"],
        })
    }

    fn run(&self, arguments: &Value, ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let (path, contents) = match (arg(arguments, "path"), arg(arguments, "contents")) {
            (Ok(path), Ok(contents)) => (path, contents),
            (Err(output), _) | (_, Err(output)) => return output,
        };
        match ops.write(Path::new(path), contents) {
            Ok(()) => Output::ok(format!("wrote {} ({} bytes)", path, contents.len())),
            Err(why) => Output::error(why),
        }
    }
}

/// Replace one exact span of a file.
pub struct Edit;

impl Tool for Edit {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file. The string must appear exactly once — include \
         enough surrounding context to make it unique."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path, relative to the session." },
                "old": { "type": "string", "description": "Exact text to replace." },
                "new": { "type": "string", "description": "What to replace it with." },
            },
            "required": ["path", "old", "new"],
        })
    }

    fn run(&self, arguments: &Value, ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let (path, old, new) = match (
            arg(arguments, "path"),
            arg(arguments, "old"),
            arg(arguments, "new"),
        ) {
            (Ok(path), Ok(old), Ok(new)) => (path, old, new),
            (Err(output), _, _) | (_, Err(output), _) | (_, _, Err(output)) => return output,
        };

        let contents = match ops.read(Path::new(path)) {
            Ok(contents) => contents,
            Err(why) => return Output::error(why),
        };

        // Counted rather than replaced-first: an ambiguous match silently patching the wrong
        // occurrence is the failure that costs an hour to find.
        let matches = contents.matches(old).count();
        match matches {
            0 => Output::error(format!(
                "that exact text is not in {path}. Read it again — it may have changed, or the \
                 whitespace may differ."
            )),
            1 => {
                let patched = contents.replacen(old, new, 1);
                match ops.write(Path::new(path), &patched) {
                    Err(why) => Output::error(why),
                    Ok(()) => Output::ok(format!("edited {path}\n{}", diff(old, new))),
                }
            }
            n => Output::error(format!(
                "that text appears {n} times in {path}. Include more surrounding context so it \
                 matches exactly once."
            )),
        }
    }
}

/// A unified diff of one replacement, for the transcript.
fn diff(old: &str, new: &str) -> String {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        out.push_str(&format!("{sign}{change}"));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;
    use crate::ops::Real;
    use std::path::PathBuf;

    fn session(name: &str) -> (Registry, Real, PathBuf) {
        let dir = std::env::temp_dir().join(format!("axum-builtin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let mut registry = Registry::new();
        install(&mut registry);
        (registry, Real::new(dir.clone()), dir)
    }

    fn call(registry: &Registry, ops: &Real, name: &str, args: Value) -> Output {
        registry.call(name, &args, ops, &crate::Uncancelled)
    }

    #[test]
    fn the_floor_is_three_tools() {
        let (registry, _, dir) = session("floor");
        assert_eq!(registry.len(), 3);
        for name in ["read", "write", "edit"] {
            assert!(registry.get(name).is_some(), "{name} is missing");
        }
        assert!(
            registry.get("bash").is_none(),
            "bash is a declared tool, not part of the floor"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_then_read_round_trips_with_line_numbers() {
        let (registry, ops, dir) = session("roundtrip");
        let written = call(
            &registry,
            &ops,
            "write",
            json!({ "path": "a.txt", "contents": "one\ntwo\n" }),
        );
        assert!(!written.is_error, "{}", written.content);

        let read = call(&registry, &ops, "read", json!({ "path": "a.txt" }));
        assert!(read.content.contains("     1\tone"), "{}", read.content);
        assert!(read.content.contains("     2\ttwo"), "{}", read.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_argument_says_which_one() {
        let (registry, ops, dir) = session("args");
        let output = call(&registry, &ops, "read", json!({}));
        assert!(output.is_error);
        assert!(output.content.contains("path"), "{}", output.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_replaces_one_exact_span() {
        let (registry, ops, dir) = session("edit");
        call(
            &registry,
            &ops,
            "write",
            json!({ "path": "a.rs", "contents": "let x = 1;\n" }),
        );
        let output = call(
            &registry,
            &ops,
            "edit",
            json!({ "path": "a.rs", "old": "let x = 1;", "new": "let x = 2;" }),
        );
        assert!(!output.is_error, "{}", output.content);
        let read = call(&registry, &ops, "read", json!({ "path": "a.rs" }));
        assert!(read.content.contains("let x = 2;"), "{}", read.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_refuses_an_ambiguous_match_rather_than_guessing() {
        // Silently patching the wrong occurrence is the failure that costs an hour to find.
        let (registry, ops, dir) = session("ambiguous");
        call(
            &registry,
            &ops,
            "write",
            json!({ "path": "a.rs", "contents": "x\nx\n" }),
        );
        let output = call(
            &registry,
            &ops,
            "edit",
            json!({ "path": "a.rs", "old": "x", "new": "y" }),
        );
        assert!(output.is_error);
        assert!(output.content.contains("2 times"), "{}", output.content);

        let read = call(&registry, &ops, "read", json!({ "path": "a.rs" }));
        assert!(!read.content.contains('y'), "nothing was changed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_says_so_when_the_text_is_not_there() {
        let (registry, ops, dir) = session("absent");
        call(
            &registry,
            &ops,
            "write",
            json!({ "path": "a.rs", "contents": "hello\n" }),
        );
        let output = call(
            &registry,
            &ops,
            "edit",
            json!({ "path": "a.rs", "old": "goodbye", "new": "hi" }),
        );
        assert!(output.is_error);
        assert!(output.content.contains("not in"), "{}", output.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_shows_a_diff_of_what_changed() {
        let (registry, ops, dir) = session("diff");
        call(
            &registry,
            &ops,
            "write",
            json!({ "path": "a.rs", "contents": "old\n" }),
        );
        let output = call(
            &registry,
            &ops,
            "edit",
            json!({ "path": "a.rs", "old": "old", "new": "new" }),
        );
        assert!(output.content.contains("-old"), "{}", output.content);
        assert!(output.content.contains("+new"), "{}", output.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_outside_the_session_is_refused_by_every_tool() {
        let (registry, ops, dir) = session("escape");
        for (name, args) in [
            ("read", json!({ "path": "../../etc/passwd" })),
            ("write", json!({ "path": "../../tmp/x", "contents": "x" })),
            (
                "edit",
                json!({ "path": "../../etc/passwd", "old": "a", "new": "b" }),
            ),
        ] {
            let output = call(&registry, &ops, name, args);
            assert!(output.is_error, "{name} allowed an escape");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_file_is_truncated_with_a_count() {
        let (registry, ops, dir) = session("long");
        let body: String = (0..PREVIEW_LINES + 50).map(|i| format!("{i}\n")).collect();
        call(
            &registry,
            &ops,
            "write",
            json!({ "path": "big.txt", "contents": body }),
        );
        let read = call(&registry, &ops, "read", json!({ "path": "big.txt" }));
        assert!(
            read.content.contains("50 more lines"),
            "truncation is stated"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
