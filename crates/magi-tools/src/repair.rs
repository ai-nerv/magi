//! Making sense of the JSON a model actually emitted. Models get two things wrong often enough to
//! repair rather than report: a raw control character inside a string — a literal newline in an
//! `edit` call's `content` — and an invalid backslash escape out of a regex or a Windows path.
//! Nothing here changes what the document says; anything still unparseable is reported.

/// The escapes JSON actually defines. A backslash before anything else was forgotten, not meant.
const ESCAPES: [char; 8] = ['"', '\\', '/', 'b', 'f', 'n', 'r', 't'];

/// Parse the arguments of a call, repairing what models get wrong.
///
/// # Errors
/// When it still does not parse, with the message the model is shown.
pub fn arguments(raw: &str) -> Result<serde_json::Value, String> {
    let text = raw.trim();
    // A call with no arguments at all is an empty object, not a failure: providers differ.
    if text.is_empty() {
        return Ok(serde_json::json!({}));
    }
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    let mended = mend(text);
    serde_json::from_str(&mended).map_err(|why| {
        format!(
            "the arguments were not valid JSON: {why}. Send the call again with the arguments \
             as one JSON object, escaping any newline inside a string as \\n."
        )
    })
}

/// Escape what a model left raw inside a string. One pass, tracking whether the cursor is inside a
/// string literal; everything outside one is passed through untouched.
#[must_use]
pub fn mend(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut inside = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                inside = !inside;
                out.push(c);
            }
            // A backslash inside a string either starts an escape or is one the model forgot to
            // double. Outside a string it is left where it is.
            '\\' if inside => match chars.peek() {
                Some(next) if ESCAPES.contains(next) || *next == 'u' => {
                    out.push(c);
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                _ => out.push_str("\\\\"),
            },
            // A literal newline in a JSON string is invalid; escaped, it is the string the model meant.
            '\n' if inside => out.push_str("\\n"),
            '\r' if inside => out.push_str("\\r"),
            '\t' if inside => out.push_str("\\t"),
            other if inside && (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json_is_left_exactly_as_it_was() {
        let value = arguments(r#"{"path":"a.rs","line":3}"#).expect("parses");
        assert_eq!(value["path"], "a.rs");
        assert_eq!(value["line"], 3);
    }

    #[test]
    fn a_raw_newline_inside_a_string_is_escaped() {
        // The common one: a model writing code into an `edit` call puts the newlines in raw.
        let value = arguments("{\"content\":\"one\ntwo\"}").expect("repaired");
        assert_eq!(
            value["content"], "one\ntwo",
            "and it is still the same string"
        );
    }

    #[test]
    fn a_tab_and_a_carriage_return_go_the_same_way() {
        let value = arguments("{\"a\":\"x\ty\",\"b\":\"p\rq\"}").expect("repaired");
        assert_eq!(value["a"], "x\ty");
        assert_eq!(value["b"], "p\rq");
    }

    #[test]
    fn a_backslash_that_is_not_an_escape_becomes_a_backslash() {
        // Out of a regex, and out of a Windows path. Both are plainly a literal backslash.
        let value = arguments(r#"{"pattern":"\d+","path":"C:\Users"}"#).expect("repaired");
        assert_eq!(value["pattern"], r"\d+");
        assert_eq!(value["path"], r"C:\Users");
    }

    #[test]
    fn a_real_escape_survives_the_repair() {
        // The repair must not double a backslash that was already doing its job.
        let value = arguments(r#"{"a":"one\ntwo","b":"say \"hi\"","c":"\u0041"}"#).expect("parses");
        assert_eq!(value["a"], "one\ntwo");
        assert_eq!(value["b"], "say \"hi\"");
        assert_eq!(value["c"], "A");
    }

    #[test]
    fn a_newline_between_fields_is_not_touched() {
        // Outside a string a newline is whitespace and already valid.
        let value = arguments("{\n  \"path\": \"a.rs\"\n}").expect("parses");
        assert_eq!(value["path"], "a.rs");
    }

    #[test]
    fn nothing_at_all_is_an_empty_object() {
        assert_eq!(arguments("").expect("empty"), serde_json::json!({}));
        assert_eq!(arguments("   ").expect("blank"), serde_json::json!({}));
    }

    #[test]
    fn something_beyond_repair_says_so_rather_than_becoming_nothing() {
        // A truncated turn produces this and nothing can be done about it. Saying so is the point:
        // the model was handed `null` before, which reads as "you asked for nothing".
        let why = arguments(r#"{"path": "a.rs"#).expect_err("truncated");
        assert!(why.contains("not valid JSON"), "{why}");
        assert!(why.contains("JSON object"), "and says what to do: {why}");
    }

    #[test]
    fn a_repair_never_changes_what_the_document_says() {
        // The rule that makes this safe. Every character out is the character in, or its escape.
        for (given, key, meant) in [
            ("{\"a\":\"x\ny\"}", "a", "x\ny"),
            (r#"{"a":"x\qy"}"#, "a", r"x\qy"),
            ("{\"a\":\"x\u{7}y\"}", "a", "x\u{7}y"),
        ] {
            assert_eq!(arguments(given).expect("repaired")[key], meant, "{given}");
        }
    }
}
