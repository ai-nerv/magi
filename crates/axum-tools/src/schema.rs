//! Checking a call against the schema that was published for it.
//!
//! Every tool declares a JSON Schema and, until this module, nothing ever read one back. A model
//! that sent a string where a number belonged, or left out a required field, got as far as the
//! tool — which failed for whatever reason it happened to fail for, in words about its own
//! internals rather than about the call. The schema was documentation.
//!
//! **Coerce, then complain.** A model that sends `"3"` for an integer has not made a mistake worth
//! a round trip; providers stringify numbers on their own and the intent is not in doubt. So a
//! primitive that can be read as the declared type is read as it, and only what cannot be is
//! reported. Pi draws the line in the same place (`utils/validation.ts:302-330`).
//!
//! **The message is the product.** It is fed straight back to the model, so it is `path: reason`,
//! one per line, naming the field — which is the difference between a model that fixes the call
//! and one that sends it again unchanged.
//!
//! **A subset, deliberately.** `type`, `required`, `properties`, `items`, `enum`, `minimum` and
//! `maximum` are what tool schemas are made of; a full validator would be a dependency and a
//! second language to be wrong in. Anything this does not understand is left alone rather than
//! refused, so a peer that declares something exotic still gets its call.

use serde_json::Value;

/// Check `arguments` against `schema`, coercing what can be coerced.
///
/// Returns the arguments as the tool should receive them.
///
/// # Errors
/// One `path: reason` line per problem, in the order they were found — every problem rather than
/// the first, because a model told about one field at a time takes one round trip per field.
pub fn check(arguments: &Value, schema: &Value) -> Result<Value, String> {
    let mut wrong = Vec::new();
    let checked = walk("", arguments, schema, &mut wrong);
    if wrong.is_empty() {
        return Ok(checked);
    }
    Err(wrong.join("\n"))
}

/// Check one value, collecting what is wrong with it and returning it coerced.
fn walk(path: &str, value: &Value, schema: &Value, wrong: &mut Vec<String>) -> Value {
    let Some(schema) = schema.as_object() else {
        return value.clone();
    };
    let named = if path.is_empty() { "arguments" } else { path };

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        let shown: Vec<String> = allowed.iter().map(ToString::to_string).collect();
        wrong.push(format!("{named}: must be one of {}", shown.join(", ")));
        return value.clone();
    }

    let Some(kind) = schema.get("type").and_then(Value::as_str) else {
        return value.clone();
    };
    let coerced = match coerce(value, kind) {
        Some(coerced) => coerced,
        None => {
            wrong.push(format!(
                "{named}: expected {}, got {}",
                article(kind),
                name(value)
            ));
            return value.clone();
        }
    };

    match kind {
        "object" => object(path, &coerced, schema, wrong),
        "array" => array(path, &coerced, schema, wrong),
        "integer" | "number" => {
            bounds(named, &coerced, schema, wrong);
            coerced
        }
        _ => coerced,
    }
}

/// Check the fields of an object, and that the required ones are there.
fn object(
    path: &str,
    value: &Value,
    schema: &serde_json::Map<String, Value>,
    wrong: &mut Vec<String>,
) -> Value {
    let Some(fields) = value.as_object() else {
        return value.clone();
    };
    let properties = schema.get("properties").and_then(Value::as_object);

    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !fields.contains_key(name) {
            wrong.push(format!("{}: required, and missing", under(path, name)));
        }
    }

    let mut out = fields.clone();
    if let Some(properties) = properties {
        for (name, given) in fields {
            // A field the schema says nothing about is left alone. Tool schemas are rarely
            // closed, and refusing an extra key would fail calls that would have worked.
            if let Some(rule) = properties.get(name) {
                out.insert(name.clone(), walk(&under(path, name), given, rule, wrong));
            }
        }
    }
    Value::Object(out)
}

/// Check the elements of an array.
fn array(
    path: &str,
    value: &Value,
    schema: &serde_json::Map<String, Value>,
    wrong: &mut Vec<String>,
) -> Value {
    let (Some(items), Some(rule)) = (value.as_array(), schema.get("items")) else {
        return value.clone();
    };
    Value::Array(
        items
            .iter()
            .enumerate()
            .map(|(at, item)| walk(&format!("{path}[{at}]"), item, rule, wrong))
            .collect(),
    )
}

/// Check `minimum` and `maximum`, which is what a tool schema uses them for: a limit or a timeout
/// with a ceiling somebody chose.
fn bounds(
    named: &str,
    value: &Value,
    schema: &serde_json::Map<String, Value>,
    wrong: &mut Vec<String>,
) {
    let Some(number) = value.as_f64() else { return };
    if let Some(low) = schema.get("minimum").and_then(Value::as_f64)
        && number < low
    {
        wrong.push(format!("{named}: must be at least {low}"));
    }
    if let Some(high) = schema.get("maximum").and_then(Value::as_f64)
        && number > high
    {
        wrong.push(format!("{named}: must be at most {high}"));
    }
}

/// Read `value` as `kind`, or say it cannot be.
///
/// The coercions are the ones a provider causes rather than the ones a model means: numbers
/// arriving as strings, and integers arriving as whole floats. Nothing here invents a value —
/// an empty string does not become `0`, and a number does not become `true`.
fn coerce(value: &Value, kind: &str) -> Option<Value> {
    match (kind, value) {
        ("string", Value::String(_))
        | ("boolean", Value::Bool(_))
        | ("object", Value::Object(_))
        | ("array", Value::Array(_))
        | ("null", Value::Null) => Some(value.clone()),
        ("number", Value::Number(_)) => Some(value.clone()),
        ("integer", Value::Number(n)) => {
            if n.is_i64() || n.is_u64() {
                return Some(value.clone());
            }
            // `3.0` is the integer 3 arriving through a float. `3.5` is not an integer and
            // rounding it would run the tool on a number nobody asked for.
            let float = n.as_f64()?;
            (float.fract() == 0.0).then(|| Value::from(float as i64))
        }
        ("string", Value::Number(n)) => Some(Value::String(n.to_string())),
        ("string", Value::Bool(b)) => Some(Value::String(b.to_string())),
        ("integer", Value::String(text)) => text.trim().parse::<i64>().ok().map(Value::from),
        ("number", Value::String(text)) => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(|n| serde_json::Number::from_f64(n).map(Value::Number)),
        ("boolean", Value::String(text)) => match text.trim() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}

/// A path a model can act on: `edit.old`, `files[2].path`.
fn under(path: &str, name: &str) -> String {
    if path.is_empty() {
        return name.to_owned();
    }
    format!("{path}.{name}")
}

/// The type name, with the article, for a sentence.
fn article(kind: &str) -> String {
    match kind {
        "object" | "array" | "integer" => format!("an {kind}"),
        other => format!("a {other}"),
    }
}

/// What a value is, in the same words the schema uses.
fn name(value: &Value) -> &'static str {
    match value {
        Value::Null => "nothing",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                "deep": { "type": "boolean" },
                "how": { "type": "string", "enum": ["fast", "slow"] },
                "names": { "type": "array", "items": { "type": "string" } },
            },
            "required": ["path"],
        })
    }

    #[test]
    fn a_call_that_fits_is_returned_unchanged() {
        let given = serde_json::json!({ "path": "a.rs", "limit": 10 });
        assert_eq!(check(&given, &schema()).expect("fits"), given);
    }

    #[test]
    fn a_missing_required_field_is_named() {
        let why = check(&serde_json::json!({ "limit": 3 }), &schema()).expect_err("missing");
        assert_eq!(why, "path: required, and missing");
    }

    #[test]
    fn a_wrong_type_says_what_it_wanted_and_what_it_got() {
        let given = serde_json::json!({ "path": ["a.rs"] });
        let why = check(&given, &schema()).expect_err("wrong");
        assert_eq!(why, "path: expected a string, got an array");
    }

    #[test]
    fn every_problem_is_reported_rather_than_the_first() {
        // A model told about one field at a time takes one round trip per field.
        let given = serde_json::json!({ "limit": "lots", "deep": "maybe" });
        let why = check(&given, &schema()).expect_err("wrong");
        assert_eq!(why.lines().count(), 3, "{why}");
    }

    #[test]
    fn a_number_sent_as_a_string_is_read_as_a_number() {
        // Providers stringify numbers on their own and the intent is not in doubt.
        let given = serde_json::json!({ "path": "a.rs", "limit": "7" });
        let fixed = check(&given, &schema()).expect("coerced");
        assert_eq!(fixed["limit"], 7);
    }

    #[test]
    fn a_whole_float_is_read_as_the_integer_it_is() {
        let given = serde_json::json!({ "path": "a.rs", "limit": 3.0 });
        assert_eq!(check(&given, &schema()).expect("coerced")["limit"], 3);
    }

    #[test]
    fn a_fraction_is_not_rounded_into_an_integer() {
        // Rounding would run the tool on a number nobody asked for.
        let given = serde_json::json!({ "path": "a.rs", "limit": 3.5 });
        let why = check(&given, &schema()).expect_err("not an integer");
        assert!(why.contains("limit"), "{why}");
    }

    #[test]
    fn a_boolean_sent_as_a_word_is_read_as_a_boolean() {
        let given = serde_json::json!({ "path": "a.rs", "deep": "true" });
        assert_eq!(check(&given, &schema()).expect("coerced")["deep"], true);
    }

    #[test]
    fn a_number_out_of_bounds_says_which_bound() {
        let over = serde_json::json!({ "path": "a.rs", "limit": 500 });
        assert_eq!(
            check(&over, &schema()).expect_err("over"),
            "limit: must be at most 100"
        );
        let under = serde_json::json!({ "path": "a.rs", "limit": 0 });
        assert_eq!(
            check(&under, &schema()).expect_err("under"),
            "limit: must be at least 1"
        );
    }

    #[test]
    fn a_value_outside_an_enum_lists_what_was_allowed() {
        let given = serde_json::json!({ "path": "a.rs", "how": "sideways" });
        let why = check(&given, &schema()).expect_err("not allowed");
        assert!(why.contains("fast"), "{why}");
        assert!(why.contains("slow"), "{why}");
    }

    #[test]
    fn an_element_of_an_array_is_named_by_its_index() {
        let given = serde_json::json!({ "path": "a.rs", "names": ["a", 2, "c"] });
        let fixed = check(&given, &schema()).expect("coerced");
        assert_eq!(
            fixed["names"][1], "2",
            "a number in a string array is read as one"
        );

        let given = serde_json::json!({ "path": "a.rs", "names": ["a", { "b": 1 }] });
        let why = check(&given, &schema()).expect_err("wrong");
        assert!(why.starts_with("names[1]:"), "{why}");
    }

    #[test]
    fn a_field_the_schema_says_nothing_about_is_left_alone() {
        // Tool schemas are rarely closed, and refusing an extra key would fail calls that would
        // otherwise have worked.
        let given = serde_json::json!({ "path": "a.rs", "extra": { "anything": true } });
        let out = check(&given, &schema()).expect("allowed");
        assert_eq!(out["extra"]["anything"], true);
    }

    #[test]
    fn a_schema_that_says_nothing_checks_nothing() {
        // A peer may declare something this subset does not understand, and a call it would
        // have answered must not be refused on our account.
        let given = serde_json::json!({ "whatever": 1 });
        assert_eq!(
            check(&given, &serde_json::json!(true)).expect("open"),
            given
        );
        assert_eq!(
            check(&given, &serde_json::json!({ "oneOf": [] })).expect("open"),
            given
        );
    }

    #[test]
    fn arguments_that_are_not_an_object_at_all_say_so() {
        // What a truncated call and a confused model both produce.
        let why = check(&Value::Null, &schema()).expect_err("not an object");
        assert_eq!(why, "arguments: expected an object, got nothing");
    }
}
