//! Telling the siblings what to be.
//!
//! magi coordinates. A sibling it started should not be reading a configuration of its own and
//! hoping the two agree, so magi asks what each takes and says — once, on the way up, before
//! either is used for anything.
//!
//! **Asked before told.** A coordinator that pushed settings from a list of its own would have
//! to know every sibling's vocabulary by heart and would be wrong first: a renamed setting would
//! fail silently on the far side and nothing here would notice. So [`needs`] is read, what magi
//! has an answer for is sent, and anything the sibling would not take comes back named.
//!
//! A sibling nobody started this way reads its own files exactly as before. This is what happens
//! when somebody is coordinating, not instead of it.

use magi_proto::setup::{Applied, Need};

/// What a sibling says it takes.
///
/// Empty when it is not installed or will not answer — not an error. A sibling that cannot be
/// asked cannot be told either, and the session carries on without it exactly as it did before
/// any of this existed.
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
    rows(&out.stdout)
        .into_iter()
        .filter_map(|row| serde_json::from_value(row).ok())
        .collect()
}

/// Hand a sibling a chunk of its own config Lua.
///
/// # Errors
/// When the sibling could not be started, would not answer, or refused the chunk outright. A
/// setting it declined is *not* an error: it comes back in [`Applied::refused`], because a
/// coordinator wants to know which one rather than have the whole exchange fail.
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
        let _ = stdin.write_all(source.as_bytes()).await;
        // Closed, because the far side reads to end of file. A handle left open is a sibling
        // waiting for a chunk that has already been written.
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
    reply
        .get("result")
        .and_then(serde_json::Value::as_array)
        .and_then(|rows| rows.first().cloned())
        .and_then(|row| serde_json::from_value(row).ok())
        .ok_or_else(|| format!("{program} answered something unreadable"))
}

/// Write the Lua that says what magi has decided, for the settings this sibling takes.
///
/// Only what it asked for. A coordinator that sent everything it knew would be relying on the
/// far side to ignore the rest, and "ignored" is indistinguishable from "misspelled".
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

/// One JSON value as the Lua literal for it.
///
/// Enough for what a coordinator sends: a string, a number, a flag. A table is declared by a
/// registrar rather than assigned, so it does not come through here.
fn lua(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => format!("{text:?}"),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        // Anything else is not a setting a coordinator should be assigning, and `nil` is the
        // honest rendering: it sets nothing and the sibling reports nothing set.
        _ => "nil".to_owned(),
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
        // The whole point of asking first. Sending a setting it does not take would be relying
        // on it to ignore the rest, and "ignored" reads the same as "misspelled".
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
        // A value with a quote in it would otherwise end the literal and leave the rest of the
        // line as Lua — which is a coordinator writing code it did not mean to.
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
