//! Making sense of the JSON a model actually emitted.
//!
//! A tool call arrives as a string the model streamed, and models get two things wrong often
//! enough to be worth handling rather than reporting: a **raw control character inside a string**
//! — a literal newline in a `content` argument is the common one, and writing code into an `edit`
//! call is where it happens — and an **invalid backslash escape**, `\d` from a regex or a Windows
//! path written `C:\Users`. Both are one character from valid, and both make `serde_json` refuse
//! the whole document.
//!
//! What that cost before this module: the parse error was thrown away and the tool was handed
//! `null` **as if the model had asked for nothing**. The tool then failed for an unrelated reason
//! — "read needs a path" — and nothing anywhere said the arguments had not parsed, so the model
//! made the same mistake again. Pi repairs the same two cases for the same reason
//! (`ai/src/utils/json-parse.ts:31-60`).
//!
//! **Repair is not guessing.** Nothing here changes what the document *says*: a raw newline
//! becomes `\n`, which is the same newline, and a stray backslash becomes a literal backslash,
//! which is what it plainly was. Anything still unparseable afterwards is reported, because a
//! model told its JSON was malformed can fix it and a model handed `null` cannot.

/// The escapes JSON actually defines.
///
/// A backslash in front of anything else is not an escape — it is a backslash somebody forgot to
/// double, which is why `\d` and `C:\Users` are the two that turn up.
const ESCAPES: [char; 8] = ['"', '\\', '/', 'b', 'f', 'n', 'r', 't'];

/// Parse the arguments of a call, repairing what models get wrong.
///
/// # Errors
/// When it still does not parse, with the message the model is shown. A truncated turn produces
/// one of these and nothing can be done about it — but saying so is the point.
pub fn arguments(raw: &str) -> Result<serde_json::Value, String> {
    let text = raw.trim();
    // A call with no arguments at all is an empty object, not a failure. Providers differ on
    // whether they send `{}` or nothing, and a tool that takes no arguments is called both ways.
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

/// Escape what a model left raw inside a string.
///
/// One pass, tracking whether the cursor is inside a string literal. Everything outside one is
/// passed through untouched: the structure is the model's and this is not the place to guess at
/// a missing brace.
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
            // double. Outside a string it is not JSON at all and is left where it is, because
            // the error should name the structure rather than a repair nobody asked for.
            '\\' if inside => match chars.peek() {
                Some(next) if ESCAPES.contains(next) || *next == 'u' => {
                    out.push(c);
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                _ => out.push_str("\\\\"),
            },
            // The whole point. A literal newline in a JSON string is invalid; the same newline
            // written `\n` is the string the model meant.
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
        // `\d` out of a regex, and `C:\Users` out of a path. Both are plainly a literal
        // backslash and neither is valid JSON.
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
        // Outside a string a newline is whitespace and already valid. Escaping it would make a
        // pretty-printed object unparseable.
        let value = arguments("{\n  \"path\": \"a.rs\"\n}").expect("parses");
        assert_eq!(value["path"], "a.rs");
    }

    #[test]
    fn nothing_at_all_is_an_empty_object() {
        // Providers differ on whether a no-argument call sends `{}` or nothing, and a tool that
        // takes no arguments is called both ways.
        assert_eq!(arguments("").expect("empty"), serde_json::json!({}));
        assert_eq!(arguments("   ").expect("blank"), serde_json::json!({}));
    }

    #[test]
    fn something_beyond_repair_says_so_rather_than_becoming_nothing() {
        // A truncated turn produces this and nothing can be done about it. Saying so is the
        // point: the model was handed `null` before, which reads as "you asked for nothing".
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
