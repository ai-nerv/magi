use magi_host::skills::{ENTRIES_MOST, FILE_MOST, METADATA_MOST, Roots, invocation, parse};
use sha2::{Digest, Sha256};
use std::path::Path;

const REFERENCE: &str = include_str!("../../../WORKFLOWS.md");

fn example(language: &str) -> Option<&str> {
    let (_, remaining) = REFERENCE.split_once(&format!("```{language}\n"))?;
    remaining.split_once("```").map(|(example, _)| example)
}

#[test]
fn documented_workflow_parses_with_exact_body_and_source_digest() {
    let source = example("markdown").expect("documented workflow example");
    let document = parse("review", source.as_bytes()).expect("documented workflow");
    assert_eq!(document.name, "review");
    assert_eq!(
        document.description,
        "Review changes: correctness and tests"
    );
    assert_eq!(
        document.body,
        "Review the requested files for correctness and missing tests.\n\
         Explain findings before proposing edits.\n"
    );
    assert_eq!(
        document.source_digest,
        format!("{:x}", Sha256::digest(source.as_bytes()))
    );
}

#[test]
fn documented_print_prompt_preserves_shell_punctuation_as_data() {
    let prompt = example("sh")
        .expect("documented print example")
        .strip_prefix("magi -p '")
        .and_then(|value| value.strip_suffix("'\n"))
        .expect("single-quoted print prompt");
    assert!(!prompt.contains('\''), "example must remain single-quoted");
    let invocation = invocation(prompt)
        .expect("valid invocation")
        .expect("workflow invocation");
    assert_eq!(invocation.name, "review");
    assert_eq!(invocation.arguments, "src/lib.rs; $(echo literal)");
}

#[test]
fn documented_root_order_matches_native_selection() {
    let roots = Roots::new(Path::new("/WORKSPACE"), None, Path::new("/CONFIG_HOME"))
        .expect("reference roots");
    let mut remaining = REFERENCE;
    for (index, source) in roots.sources().iter().enumerate() {
        let root = source.root.strip_prefix("/").expect("absolute root");
        let row = format!("| {} | `{}` |", index + 1, root.display());
        remaining = remaining
            .split_once(&row)
            .expect("ordered documented root")
            .1;
    }
}

#[test]
fn documented_byte_limits_match_native_constants() {
    for row in [
        format!("| Entries per root | {ENTRIES_MOST} |"),
        format!("| Metadata, including delimiters and line endings | {METADATA_MOST} bytes |"),
        format!("| Complete selected file, including metadata | {FILE_MOST} bytes |"),
    ] {
        assert!(REFERENCE.contains(&row), "missing reference row: {row}");
    }
}
