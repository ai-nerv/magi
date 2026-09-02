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

/// Bytes of a file shown, whichever limit is reached first.
///
/// A minified bundle, a lockfile or a generated single-line JSON is one line and megabytes, so
/// a line count alone is not a bound. The chokepoint in [`crate::bound`] would catch it, but
/// catching it here means the model is told to *page* rather than handed a middle-cut blob.
const PREVIEW_BYTES: usize = 50_000;

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

/// An optional non-negative integer argument.
fn count(arguments: &Value, name: &str) -> Option<usize> {
    arguments[name]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
}

/// Read a file.
pub struct Read;

impl Tool for Read {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a file from the session's directory. Returns the contents with line numbers, \
         which is what `edit` matches against.\n\n\
         Long files are truncated. Pass `offset` to continue from where the last read stopped, \
         and keep going until you have what you need."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path, relative to the session." },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "First line to show, 1-based. Defaults to the start.",
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "How many lines at most. Defaults to as many as fit.",
                },
            },
            "required": ["path"],
        })
    }

    fn run(&self, arguments: &Value, ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let path = match arg(arguments, "path") {
            Ok(path) => path,
            Err(output) => return output,
        };
        // Asked before it is done, and in the terms the person will see: "read /etc/shadow"
        // is a question somebody can answer, "the read tool wants to run" is not.
        if let Err(why) = ops.allow(
            "read",
            &magi_proto::permit::Action::Read {
                path: ops.cwd().join(path).display().to_string(),
            },
        ) {
            return Output::error(why);
        }
        match ops.read(Path::new(path)) {
            Err(why) => Output::error(why),
            Ok(contents) => {
                let lines: Vec<&str> = contents.lines().collect();
                // 1-based on the wire because that is what the numbers in the output say, and a
                // model asked to continue from line 2001 should be able to type 2001.
                let start = count(arguments, "offset").unwrap_or(1).max(1) - 1;
                if start >= lines.len() && !lines.is_empty() {
                    return Output::error(format!(
                        "offset {} is past the end; the file has {} lines",
                        start + 1,
                        lines.len()
                    ));
                }
                let budget = count(arguments, "limit").unwrap_or(PREVIEW_LINES);

                let mut out = String::new();
                let mut shown = 0usize;
                for (index, line) in lines[start..].iter().enumerate() {
                    if shown >= budget || out.len() + line.len() > PREVIEW_BYTES {
                        break;
                    }
                    // Numbered because `edit` matches on content and a model that can count
                    // lines makes better patches than one guessing at them.
                    out.push_str(&format!("{:>6}\t{line}\n", start + index + 1));
                    shown += 1;
                }

                let next = start + shown;
                if next < lines.len() {
                    out.push_str(&format!(
                        "… {} more lines. Continue with offset={}\n",
                        lines.len() - next,
                        next + 1
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
        if let Err(why) = ops.allow(
            "write",
            &magi_proto::permit::Action::Write {
                path: ops.cwd().join(path).display().to_string(),
            },
        ) {
            return Output::error(why);
        }
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
                if let Err(why) = ops.allow(
                    "edit",
                    &magi_proto::permit::Action::Write {
                        path: ops.cwd().join(path).display().to_string(),
                    },
                ) {
                    return Output::error(why);
                }
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
        let dir = std::env::temp_dir().join(format!("magi-builtin-{}-{name}", std::process::id()));
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
    fn a_confined_session_refuses_an_escape_from_every_tool() {
        // The rule lives on `magi.confine` now, and this is what it buys when it is on.
        let dir = std::env::temp_dir().join(format!("magi-bwall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let mut registry = crate::Registry::new();
        install(&mut registry);
        let ops = crate::ops::Real::confined(dir.clone());
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

#[cfg(test)]
mod paging_tests {
    use super::*;
    use crate::cancel::Uncancelled;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-read-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn read_with(dir: &std::path::Path, args: Value) -> Output {
        Read.run(
            &args,
            &crate::ops::Real::new(dir.to_path_buf()),
            &Uncancelled,
        )
    }

    #[test]
    fn a_long_file_says_where_to_continue_from() {
        // It used to say "ask for them with a shell command", which is the unbounded path this
        // milestone exists to close.
        let dir = scratch("long");
        let body: String = (1..=5_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("big.txt"), body).expect("write");
        let out = read_with(&dir, json!({ "path": "big.txt" }));
        assert!(
            out.content.contains("Continue with offset=2001"),
            "{:.300}",
            out.content
        );
        assert!(
            !out.content.contains("shell command"),
            "no longer sends you to bash"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn continuing_from_an_offset_resumes_where_it_stopped() {
        let dir = scratch("resume");
        let body: String = (1..=5_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("big.txt"), body).expect("write");
        let out = read_with(&dir, json!({ "path": "big.txt", "offset": 2001 }));
        assert!(
            out.content.contains("  2001\tline 2001"),
            "{:.200}",
            out.content
        );
        assert!(!out.content.contains("\t line 2000"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_numbers_shown_are_the_numbers_you_pass_back() {
        // `edit` matches on content and the model counts from these; an offset that meant
        // something different from the printed number would be a trap.
        let dir = scratch("numbers");
        std::fs::write(dir.join("a.txt"), "a\nb\nc\nd\n").expect("write");
        let out = read_with(&dir, json!({ "path": "a.txt", "offset": 3 }));
        assert!(out.content.starts_with("     3\tc"), "{:?}", out.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_limit_is_honoured() {
        let dir = scratch("limit");
        let body: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("a.txt"), body).expect("write");
        let out = read_with(&dir, json!({ "path": "a.txt", "limit": 5 }));
        assert_eq!(out.content.lines().count(), 6, "five lines and the note");
        assert!(out.content.contains("Continue with offset=6"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_enormous_line_is_bounded_by_bytes() {
        // A minified bundle is one line and megabytes. A line count alone is not a bound.
        let dir = scratch("minified");
        std::fs::write(dir.join("bundle.js"), "x".repeat(PREVIEW_BYTES * 3)).expect("write");
        let out = read_with(&dir, json!({ "path": "bundle.js" }));
        assert!(
            out.content.len() < PREVIEW_BYTES * 2,
            "{} bytes",
            out.content.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_offset_past_the_end_says_so_rather_than_returning_nothing() {
        let dir = scratch("past");
        std::fs::write(dir.join("a.txt"), "a\nb\n").expect("write");
        let out = read_with(&dir, json!({ "path": "a.txt", "offset": 99 }));
        assert!(out.is_error, "{:?}", out.content);
        assert!(out.content.contains("has 2 lines"), "{}", out.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_file_gets_no_continuation_note() {
        let dir = scratch("short");
        std::fs::write(dir.join("a.txt"), "a\nb\n").expect("write");
        let out = read_with(&dir, json!({ "path": "a.txt" }));
        assert!(!out.content.contains("Continue"), "{}", out.content);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
