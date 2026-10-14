use super::*;
use serde_json::json;

fn rendered(templates: &[&str], arguments: &serde_json::Value) -> Vec<String> {
    attempted(templates, arguments).expect("rendered")
}

fn attempted(templates: &[&str], arguments: &serde_json::Value) -> Result<Vec<String>, String> {
    let templates: Vec<String> = templates.iter().map(|t| (*t).to_owned()).collect();
    render(&templates, arguments)
}

#[test]
fn a_value_that_would_be_read_as_an_option_is_refused() {
    for (templates, arguments) in [
        (vec!["{pattern}"], json!({ "pattern": "--pre=sh" })),
        (vec!["-n", "{paths}"], json!({ "paths": ["a.rs", "-rf"] })),
        (vec!["{name}.txt"], json!({ "name": "-x" })),
        (vec!["{count}"], json!({ "count": -1 })),
    ] {
        let why = attempted(&templates, &arguments).expect_err("refused");
        assert!(why.contains("begins with `-`"), "{templates:?}: {why}");
    }
}

#[test]
fn after_a_double_dash_a_value_is_a_value_whatever_it_begins_with() {
    assert_eq!(
        rendered(
            &["-n", "--", "{pattern}"],
            &json!({ "pattern": "--pre=sh" })
        ),
        ["-n", "--", "--pre=sh"]
    );
}

#[test]
fn a_value_inside_a_flag_is_that_flags_value() {
    assert_eq!(
        rendered(&["--max-count={limit}"], &json!({ "limit": -1 })),
        ["--max-count=-1"]
    );
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
