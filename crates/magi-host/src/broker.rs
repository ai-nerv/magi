//! Asking melchior, instead of asking a model.
//!
//! magi is the middle. It gathers the context — from the transcript balthasar holds — hands it
//! to melchior as an [`Ask`], and writes down what comes back. The protocols, the credentials
//! and the HTTP are melchior's, and nothing in this file knows what any of them look like.
//!
//! # Why a spawn and not a socket
//!
//! A turn is a stream, and the family socket is request and reply. One exec per turn gives the
//! stream for free — melchior writes a [`Said`] per line and exits when the turn is over — and
//! costs a process for something that already takes seconds. When a turn needs to be *steered*
//! rather than watched, this becomes a socket; until then a pipe is the honest shape.

use magi_model::Context;
use magi_model::Delta;
use magi_proto::ask::{Ask, Refusal, Said, Wants};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// What went wrong asking, when the asking itself failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    /// What to tell a person.
    pub message: String,
    /// Which kind of failure, so the turn can act rather than only report.
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
    /// How many will be made in all.
    pub max_attempts: u32,
    /// How long before the next one.
    pub delay_ms: u64,
}

/// Whether a melchior is reachable to ask.
///
/// Looked for once and not cached: a person installing it mid-session should not have to
/// restart, and the cost is a `stat` per turn.
#[must_use]
pub fn available() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join("melchior").is_file())
    })
}

/// Run one turn through melchior, reporting each delta as it arrives.
///
/// # Errors
/// When melchior could not be started or spoke something this build cannot read. A model that
/// refused is not an error here: it arrives as [`Said::Failed`] and is returned as [`Trouble`],
/// which is the same shape a provider failure had.
pub async fn ask(
    model: &str,
    context: &Context,
    wants: &Wants,
    on_delta: impl FnMut(Delta),
) -> Result<(), Trouble> {
    ask_reporting(model, context, wants, on_delta, |_| {}).await
}

/// The same, saying when melchior is waiting to try again.
///
/// A backoff is invisible from here — melchior does the waiting — and forty seconds of nothing
/// reads as a hang. This is how the status line learns to say otherwise.
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

/// The program that owns the model.
///
/// Found on `PATH`, like every other sibling. Named here rather than inline so a test can put
/// something else in its place without touching the environment: `PATH` is process-wide, and
/// tests that fought over it would be tests that pass alone and fail together.
const MELCHIOR: &str = "melchior";

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

    // JSON rather than CBOR, though melchior reads both. What crosses here is a conversation
    // and a signature, and a signature is already text; CBOR's advantage is bytes nobody is
    // going to read, and these are read constantly while this is being built.
    let mut child = tokio::process::Command::new(program)
        .arg("ask")
        .arg("--json")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|why| Trouble {
            message: format!("{program} could not be started: {why}"),
            why: Refusal::Transport,
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Written and closed. melchior reads to end of file, so a handle left open is a turn
        // that never starts.
        let _ = stdin.write_all(&body).await;
        let _ = stdin.shutdown().await;
    }

    let Some(stdout) = child.stdout.take() else {
        return Err(Trouble {
            message: "melchior gave nothing to read".to_owned(),
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
            // A line this build cannot read is not a reason to abandon a turn in progress: a
            // newer melchior may say things this one has no name for, and the answer so far is
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
                // Milliseconds, because that is what the status line shows and a float of
                // seconds would be rounded there anyway.
                delay_ms: (seconds * 1000.0) as u64,
            }),
            other => on_delta(carried(other)),
        }
    }
    let _ = child.wait().await;

    // Silence is the one answer a reader cannot interpret, so it is named here rather than
    // returned as success. A turn that ends without a terminal lost its mind mid-sentence.
    ended.unwrap_or_else(|| {
        Err(Trouble {
            message: "melchior stopped without finishing the turn".to_owned(),
            why: Refusal::Transport,
        })
    })
}

/// One [`Said`], as the turn machinery already understands it.
///
/// The inverse of melchior's own translation. `Stop` and `Failed` are handled by the caller,
/// which is why they are absent: one ends the stream and the other ends the turn.
fn carried(said: Said) -> Delta {
    match said {
        Said::Text { text } => Delta::Text(text),
        Said::Thinking { text } => Delta::Thinking(text),
        Said::Signature { signature } => Delta::Signature(signature),
        Said::ToolCallStart { id, name } => Delta::ToolCallStart { id, name },
        Said::ToolCallArgs { args } => Delta::ToolCallArgs(args),
        Said::Spent { usage } => Delta::Usage(usage),
        // Unreachable by construction: the caller takes both before this is called. Mapped to a
        // stop rather than panicking, because a `todo!()` here would be a crash in a turn.
        Said::Stop { reason } => Delta::Stop(reason),
        Said::Failed { .. } | Said::Retrying { .. } => Delta::Stop(magi_model::StopReason::Error),
    }
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
        // Nothing to ask, so this must come back rather than wait. The message names the thing
        // that is missing, because "the model did not answer" would send somebody to the wrong
        // half of the family.
        if available() {
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

/// Ask for a value rather than a conversation, and parse what comes back.
///
/// For the places magi wants a *shape* — the permissions a config declares it needs, and
/// anything else that asks the model to fill in a schema. The stream is collected rather than
/// published: nobody is watching, and half of a JSON object on screen is worse than none.
///
/// # Errors
/// Whatever [`ask`] would return, and [`Refusal::Invalid`] when the answer will not parse as the
/// shape that was asked for.
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

    // A call is preferred over prose: Anthropic answers a schema by calling a forced tool, and
    // anything in the text beside it is commentary.
    let raw = if args.trim().is_empty() { &text } else { &args };
    serde_json::from_str(raw.trim()).map_err(|why| Trouble {
        message: format!("the answer was not the shape that was asked for: {why}"),
        why: Refusal::Invalid,
    })
}

/// What melchior says this machine can talk to.
///
/// Asked once, when a session starts. A card is small and there are a few hundred of them, so
/// the cost is a process and a parse — against which the alternative is magi keeping its own
/// catalog, which is the thing that drifts.
///
/// Empty when melchior is not installed or would not answer. Not an error: a session with no
/// models says so when a prompt arrives, which is where a person can act on it.
pub async fn cards() -> Vec<magi_proto::ask::Card> {
    let Ok(out) = tokio::process::Command::new("melchior")
        .arg("models")
        .arg("--json")
        .stderr(std::process::Stdio::null())
        .output()
        .await
    else {
        return Vec::new();
    };
    let Ok(reply) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
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
