//! Asking melchior instead of asking a model: magi gathers the context, hands it over as an
//! [`Ask`], and writes down what comes back. One exec per turn rather than the family socket,
//! because a turn is a stream and the socket is request and reply.

use magi_model::Context;
use magi_model::Delta;
use magi_proto::ask::{Ask, Refusal, Said, Wants};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// What went wrong asking, when the asking itself failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    pub message: String,
    pub why: Refusal,
}

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A wait melchior is taking before trying again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    /// Which attempt just failed, counting from one.
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay_ms: u64,
}

/// Whether `program` is reachable to ask. Looked for once and not cached, so installing it
/// mid-session works. Takes the name rather than assuming [`MELCHIOR`], which `magi.melchior` moves.
#[must_use]
pub fn available(program: &str) -> bool {
    if program.contains(std::path::MAIN_SEPARATOR) {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Run one turn through melchior, reporting each delta as it arrives.
///
/// # Errors
/// When melchior could not be started or spoke something this build cannot read; a model that
/// refused is not an error here but a [`Trouble`] carrying [`Said::Failed`].
pub async fn ask(
    model: &str,
    context: &Context,
    wants: &Wants,
    on_delta: impl FnMut(Delta),
) -> Result<(), Trouble> {
    ask_reporting(model, context, wants, on_delta, |_| {}).await
}

/// The same, saying when melchior is waiting to try again — a backoff is invisible from here, and
/// forty seconds of nothing reads as a hang.
///
/// # Errors
/// As [`ask`].
pub async fn ask_reporting(
    model: &str,
    context: &Context,
    wants: &Wants,
    on_delta: impl FnMut(Delta),
    on_retry: impl FnMut(Retry),
) -> Result<(), Trouble> {
    ask_through(MELCHIOR, model, context, wants, on_delta, on_retry).await
}

/// The program that owns the model, found on `PATH`. Public because a [`crate::turn::Backend`]
/// carries the name it will ask, so a test can substitute one without touching process-wide `PATH`.
pub const MELCHIOR: &str = "melchior";

/// The same, against a named program.
///
/// # Errors
/// As [`ask`].
pub async fn ask_through(
    program: &str,
    model: &str,
    context: &Context,
    wants: &Wants,
    mut on_delta: impl FnMut(Delta),
    mut on_retry: impl FnMut(Retry),
) -> Result<(), Trouble> {
    let asking = Ask {
        model: model.to_owned(),
        context: context.clone(),
        wants: wants.clone(),
        about: String::new(),
    };
    let body = serde_json::to_vec(&asking).map_err(|why| Trouble {
        message: format!("this turn will not encode: {why}"),
        why: Refusal::Invalid,
    })?;

    // JSON rather than CBOR, though melchior reads both: what crosses here is read constantly.
    let mut child = tokio::process::Command::new(program)
        .arg("ask")
        .arg("--json")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // A dropped `tokio::process::Child` is detached rather than ended, so an interrupted ask
        // would keep running, still streaming and still billed.
        .kill_on_drop(true)
        .spawn()
        .map_err(|why| {
            magi_model::noted!("broker: {program} ask could not be started: {why}");
            Trouble {
                message: format!("{program} could not be started: {why}"),
                why: Refusal::Transport,
            }
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Written and closed: melchior reads to end of file, so a handle left open is a turn that
        // never starts.
        if let Err(why) = stdin.write_all(&body).await {
            magi_model::noted!("broker: the ask to {program} was not fully written: {why}");
        }
        let _ = stdin.shutdown().await;
    }

    let Some(stdout) = child.stdout.take() else {
        return Err(Trouble {
            message: format!("{program} gave nothing to read"),
            why: Refusal::Transport,
        });
    };

    let mut lines = BufReader::new(stdout).lines();
    let mut ended = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(said) = serde_json::from_str::<Said>(&line) else {
            // A newer melchior may say things this build has no name for, and the answer so far is
            // still the answer.
            continue;
        };
        match said {
            Said::Failed { message, why } => {
                ended = Some(Err(Trouble { message, why }));
            }
            Said::Stop { reason } => {
                on_delta(Delta::Stop(reason));
                ended = Some(Ok(()));
            }
            Said::Retrying {
                attempt,
                of,
                seconds,
                ..
            } => on_retry(Retry {
                attempt,
                max_attempts: of,
                // Milliseconds, because that is what the status line shows.
                delay_ms: (seconds * 1000.0) as u64,
            }),
            other => on_delta(carried(other)),
        }
    }
    let _ = child.wait().await;

    // A turn that ends without a terminal is named here rather than returned as success.
    ended.unwrap_or_else(|| {
        Err(Trouble {
            message: format!("{program} stopped without finishing the turn"),
            why: Refusal::Transport,
        })
    })
}

/// One [`Said`], as the turn machinery already understands it. `Stop` and `Failed` are absent
/// because the caller takes them: one ends the stream, the other ends the turn.
fn carried(said: Said) -> Delta {
    match said {
        Said::Text { text } => Delta::Text(text),
        Said::Thinking { text } => Delta::Thinking(text),
        Said::Signature { signature } => Delta::Signature(signature),
        Said::ToolCallStart { id, name } => Delta::ToolCallStart { id, name },
        Said::ToolCallArgs { args } => Delta::ToolCallArgs(args),
        Said::Spent { usage } => Delta::Usage(usage),
        // Unreachable by construction: the caller takes both before this is called.
        Said::Stop { reason } => Delta::Stop(reason),
        Said::Failed { .. } | Said::Retrying { .. } => Delta::Stop(magi_model::StopReason::Error),
    }
}

/// Ask for a value rather than a conversation, and parse what comes back. The stream is collected
/// rather than published: half of a JSON object on screen is worse than none.
///
/// # Errors
/// Whatever [`ask`] would return, and [`Refusal::Invalid`] when the answer will not parse.
pub async fn value(
    model: &str,
    context: &Context,
    wants: &Wants,
) -> Result<serde_json::Value, Trouble> {
    let mut text = String::new();
    let mut args = String::new();
    ask(model, context, wants, |delta| match delta {
        Delta::Text(chunk) => text.push_str(&chunk),
        Delta::ToolCallArgs(chunk) => args.push_str(&chunk),
        _ => {}
    })
    .await?;

    // A call is preferred over prose: Anthropic answers a schema by calling a forced tool.
    let raw = if args.trim().is_empty() { &text } else { &args };
    serde_json::from_str(raw.trim()).map_err(|why| Trouble {
        message: format!("the answer was not the shape that was asked for: {why}"),
        why: Refusal::Invalid,
    })
}

/// What melchior says this machine can talk to, asked once when a session starts. Empty when
/// melchior is not installed or would not answer, which is not an error.
pub async fn cards(program: &str) -> Vec<magi_proto::ask::Card> {
    let Ok(out) = tokio::process::Command::new(program)
        .arg("models")
        .arg("--json")
        .stderr(std::process::Stdio::null())
        .output()
        .await
    else {
        magi_model::noted!("broker: {program} models could not be started");
        return Vec::new();
    };
    let Ok(reply) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        magi_model::noted!(
            "broker: {program} models answered {} bytes that are not json",
            out.stdout.len()
        );
        return Vec::new();
    };
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        magi_model::noted!("broker: {program} models refused: {reply}");
        return Vec::new();
    }
    reply
        .get("result")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| serde_json::from_value(row.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::{StopReason, Usage};

    #[test]
    fn every_said_becomes_the_delta_that_means_the_same() {
        assert_eq!(
            carried(Said::Text { text: "a".into() }),
            Delta::Text("a".into())
        );
        assert_eq!(
            carried(Said::Thinking { text: "a".into() }),
            Delta::Thinking("a".into())
        );
        assert_eq!(
            carried(Said::Signature {
                signature: "opaque".into()
            }),
            Delta::Signature("opaque".into())
        );
        assert_eq!(
            carried(Said::ToolCallArgs { args: "{".into() }),
            Delta::ToolCallArgs("{".into())
        );
        assert_eq!(
            carried(Said::Spent {
                usage: Usage::default()
            }),
            Delta::Usage(Usage::default())
        );
    }

    #[test]
    fn a_tool_call_keeps_the_identity_the_result_must_quote_back() {
        let delta = carried(Said::ToolCallStart {
            id: "t1".into(),
            name: "shell".into(),
        });
        assert_eq!(
            delta,
            Delta::ToolCallStart {
                id: "t1".into(),
                name: "shell".into()
            }
        );
    }

    #[test]
    fn a_failure_that_reached_here_is_still_an_end_rather_than_a_panic() {
        assert_eq!(
            carried(Said::Failed {
                message: "x".into(),
                why: Refusal::Invalid
            }),
            Delta::Stop(StopReason::Error)
        );
    }

    #[tokio::test]
    async fn an_absent_melchior_is_reported_rather_than_hung() {
        // The message names the thing that is missing rather than blaming the model.
        if available(MELCHIOR) {
            return;
        }
        let trouble = ask(
            "openrouter/anything",
            &Context::default(),
            &Wants::default(),
            |_| {},
        )
        .await
        .expect_err("no melchior");
        assert!(trouble.message.contains("melchior"), "{trouble:?}");
    }
}
