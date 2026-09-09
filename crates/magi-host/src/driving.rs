//! Telling the siblings what to be. magi asks what each takes and says it once, on the way up:
//! [`needs`] is read, what magi has an answer for is sent, and anything the sibling would not take
//! comes back named. A sibling nobody started this way reads its own files exactly as before.

use magi_proto::setup::{Applied, Need};

/// What a sibling says it takes. Empty when it is not installed or will not answer, which is not
/// an error: a sibling that cannot be asked cannot be told either.
pub async fn needs(program: &str) -> Vec<Need> {
    let Ok(out) = tokio::process::Command::new(program)
        .arg("needs")
        .arg("--json")
        .stderr(std::process::Stdio::null())
        .output()
        .await
    else {
        magi_model::noted!("driving: {program} needs could not be started");
        return Vec::new();
    };
    flat(rows(&out.stdout))
        .into_iter()
        .filter_map(|row| serde_json::from_value(row).ok())
        .collect()
}

/// Hand a sibling a chunk of its own config Lua.
///
/// # Errors
/// When the sibling could not be started, would not answer, or refused the chunk outright. A
/// setting it declined is not an error: it comes back in [`Applied::refused`].
pub async fn configure(program: &str, source: &str) -> Result<Applied, String> {
    let mut child = tokio::process::Command::new(program)
        .arg("configure")
        .arg("--json")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|why| {
            magi_model::noted!("driving: {program} configure could not be started: {why}");
            format!("{program} could not be started: {why}")
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        // Half of a Lua file is still a Lua file, and the sibling would apply it; the log at least
        // says the pipe went.
        if let Err(why) = stdin.write_all(source.as_bytes()).await {
            magi_model::noted!("driving: the configuration for {program} was cut short: {why}");
        }
        // Closed, because the far side reads to end of file.
        let _ = stdin.shutdown().await;
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|why| format!("{program} did not finish: {why}"))?;

    let reply: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|why| format!("{program}: {why}"))?;
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(reply
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("it refused and gave no reason")
            .to_owned());
    }
    flat(
        reply
            .get("result")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default(),
    )
    .into_iter()
    .find_map(|row| serde_json::from_value(row).ok())
    .ok_or_else(|| format!("{program} answered something unreadable"))
}

/// Write the Lua that says what magi has decided, for the settings this sibling takes. Only what it
/// asked for: "ignored" is indistinguishable from "misspelled".
#[must_use]
pub fn saying(module: &str, needs: &[Need], answers: &[(&str, serde_json::Value)]) -> String {
    let mut out = String::new();
    for (name, value) in answers {
        if !needs.iter().any(|need| need.name == *name) {
            continue;
        }
        out.push_str(&format!("{module}.{name} = {}\n", lua(value)));
    }
    out
}

/// One JSON value as the Lua literal for it, tables included — melchior, balthasar and casper all
/// declare table settings. A map becomes `{ ["key"] = value }` and a list `{ value, value }`; keys
/// are bracketed strings, so a key that is a Lua keyword or has a dash in it is not a syntax error.
fn lua(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => format!("{text:?}"),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(lua).collect();
            format!("{{ {} }}", inner.join(", "))
        }
        serde_json::Value::Object(fields) => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(key, value)| format!("[{key:?}] = {}", lua(value)))
                .collect();
            format!("{{ {} }}", inner.join(", "))
        }
        // `nil` sets nothing, and the sibling reports nothing set.
        serde_json::Value::Null => "nil".to_owned(),
    }
}

/// The rows a reply meant, when one of them turns out to be the rows. casper once sent its listings
/// as one row that was itself a list. It sends them flat now; this stays because the four programs
/// ship from four repositories and are installed one at a time.
fn flat(rows: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    match rows.first() {
        Some(serde_json::Value::Array(inner)) if rows.len() == 1 => inner.clone(),
        _ => rows,
    }
}

/// The rows of a family reply, or nothing when it was not one.
fn rows(body: &[u8]) -> Vec<serde_json::Value> {
    let Ok(reply) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Vec::new();
    }
    reply
        .get("result")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::setup::Kind;

    fn need(name: &str, kind: Kind) -> Need {
        Need {
            name: name.to_owned(),
            kind,
            about: String::new(),
            required: false,
            default: None,
        }
    }

    #[test]
    fn only_what_the_sibling_asked_for_is_sent() {
        let needs = [need("thinking", Kind::Text)];
        let said = saying(
            "melchior",
            &needs,
            &[
                ("thinking", serde_json::json!("high")),
                ("colour", serde_json::json!("green")),
            ],
        );
        assert!(said.contains("melchior.thinking = \"high\""), "{said}");
        assert!(!said.contains("colour"), "{said}");
    }

    #[test]
    fn each_kind_is_written_as_the_lua_for_it() {
        let needs = [
            need("thinking", Kind::Text),
            need("max_tokens", Kind::Number),
            need("discover", Kind::Flag),
        ];
        let said = saying(
            "melchior",
            &needs,
            &[
                ("thinking", serde_json::json!("low")),
                ("max_tokens", serde_json::json!(256)),
                ("discover", serde_json::json!(true)),
            ],
        );
        assert!(said.contains(r#"melchior.thinking = "low""#), "{said}");
        assert!(said.contains("melchior.max_tokens = 256"), "{said}");
        assert!(said.contains("melchior.discover = true"), "{said}");
    }

    #[test]
    fn a_string_is_quoted_and_escaped_rather_than_pasted() {
        // A value with a quote in it would otherwise end the literal and leave the rest as Lua.
        let needs = [need("model", Kind::Text)];
        let said = saying(
            "melchior",
            &needs,
            &[("model", serde_json::json!(r#"a" .. os.time() .. ""#))],
        );
        assert!(!said.contains("os.time()\n"), "unescaped: {said}");
        assert!(said.contains("\\\""), "the quote is escaped: {said}");
    }

    #[test]
    fn nothing_to_say_is_an_empty_chunk_rather_than_a_broken_one() {
        assert_eq!(saying("melchior", &[], &[]), "");
    }

    #[test]
    fn a_reply_that_is_not_the_familys_shape_yields_no_rows() {
        assert!(rows(b"not json at all").is_empty());
        assert!(rows(br#"{"ok":false,"error":"no"}"#).is_empty());
        assert_eq!(rows(br#"{"ok":true,"n":1,"result":[1]}"#).len(), 1);
    }

    #[tokio::test]
    async fn a_sibling_that_is_not_there_asks_for_nothing() {
        assert!(needs("magi-no-such-sibling-anywhere").await.is_empty());
    }

    #[tokio::test]
    async fn configuring_something_absent_says_which_program() {
        let why = configure("magi-no-such-sibling-anywhere", "")
            .await
            .expect_err("nothing to configure");
        assert!(why.contains("magi-no-such-sibling-anywhere"), "{why}");
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    fn need(name: &str) -> Need {
        Need {
            name: name.to_owned(),
            kind: magi_proto::setup::Kind::Table,
            about: String::new(),
            required: false,
            default: None,
        }
    }

    #[test]
    fn a_table_setting_is_rendered_rather_than_nilled() {
        let said = saying(
            "casper",
            &[need("tools")],
            &[(
                "tools",
                serde_json::json!({ "dino": { "off": true }, "shell": { "hidden": true } }),
            )],
        );
        assert!(
            said.contains("casper.tools = {"),
            "a table, not nil: {said}"
        );
        assert!(said.contains(r#"["dino"]"#), "{said}");
        assert!(said.contains("[\"off\"] = true"), "{said}");
        assert!(!said.contains("nil"), "nothing was dropped: {said}");
    }

    #[test]
    fn a_list_is_a_list_and_not_a_map() {
        let said = saying(
            "balthasar",
            &[need("sources")],
            &[("sources", serde_json::json!(["magi", "shell"]))],
        );
        assert!(said.contains(r#"{ "magi", "shell" }"#), "{said}");
    }

    #[test]
    fn a_key_that_is_not_an_identifier_is_still_written() {
        // Bracketed strings rather than bare names: a dash or a Lua keyword in a key would
        // otherwise be a syntax error in a chunk somebody else has to run.
        let said = saying(
            "melchior",
            &[need("compat")],
            &[(
                "compat",
                serde_json::json!({ "thinking-format": "deepseek", "end": 1 }),
            )],
        );
        assert!(said.contains(r#"["thinking-format"]"#), "{said}");
        assert!(said.contains(r#"["end"]"#), "a keyword as a key: {said}");
    }
    #[test]
    fn a_listing_that_arrived_as_one_row_of_rows_is_still_read() {
        let nested = vec![serde_json::json!([{ "name": "tools" }, { "name": "load" }])];
        assert_eq!(flat(nested).len(), 2);
    }

    #[test]
    fn flat_rows_are_left_alone() {
        let rows = vec![serde_json::json!({ "name": "tools" })];
        assert_eq!(flat(rows.clone()), rows);
        // Two rows whose first is genuinely a list is not the wrapping, and unwrapping it would
        // lose the second.
        let mixed = vec![serde_json::json!([1, 2]), serde_json::json!(3)];
        assert_eq!(flat(mixed.clone()), mixed);
    }
}
