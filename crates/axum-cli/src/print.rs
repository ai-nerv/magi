//! `axum -p "…"` — one prompt, one answer, no terminal.
//!
//! The same daemon, the same journal and the same turn loop the UI drives; only the front end
//! is different. That is the point of running it through the socket rather than in-process: a
//! `-p` run leaves a session behind that `axum --resume` picks up, and there is one
//! implementation of the loop rather than two that agree until they stop agreeing.

use anyhow::Result;
use axum_ipc::{FrameReader, FrameWriter};

use axum_proto::{AgentStatus, Cursor, HarnessEvent, StopReason, UiCommand};
use std::path::Path;

/// What a finished print run reports to the shell.
///
/// An error is an exit code, not a panic: `-p` is what gets put in a pipeline, and a caller
/// deciding whether the answer is usable should not have to parse the answer to find out.
pub struct Outcome {
    /// The last assistant message, which is what goes to stdout.
    pub text: String,
    /// Why it ended, when the daemon said.
    pub stop_reason: Option<StopReason>,
    /// The failure, if there was one.
    pub error: Option<String>,
}

impl Outcome {
    /// Whether the run should exit non-zero.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.error.is_some()
            || matches!(
                self.stop_reason,
                Some(StopReason::Error | StopReason::Aborted)
            )
    }
}

/// Submit one prompt and collect the answer.
pub async fn run(socket: &Path, prompt: String) -> Result<Outcome> {
    let stream = axum_ipc::connect(socket).await?;
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    // Attached from the end rather than from zero: a resumed session's history is context for
    // the model, not output for this run, and replaying it would print an answer to somebody
    // else's question as if it were this one's.
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: FROM_END,
        })
        .await?;
    if !matches!(
        reader.read::<HarnessEvent>().await?,
        HarnessEvent::SessionSnapshot { .. }
    ) {
        anyhow::bail!("the daemon did not open with a snapshot");
    }
    writer
        .write(&UiCommand::SubmitPrompt { text: prompt })
        .await?;

    let mut text = String::new();
    let mut stop_reason = None;
    let mut error = None;
    let mut started = false;

    while let Ok(event) = reader.read::<HarnessEvent>().await {
        match event {
            HarnessEvent::AssistantStarted { .. } => {
                // Only the last message is printed, so each new one replaces what came before:
                // the intermediate messages in a tool-using turn are working, not the answer.
                text.clear();
                started = true;
            }
            HarnessEvent::AssistantDelta { text: chunk, .. } => text.push_str(&chunk),
            HarnessEvent::AssistantEnded {
                stop_reason: reason,
                error: failure,
                ..
            } => {
                stop_reason = Some(reason);
                error = failure;
                // A turn that stopped to run tools has not answered yet; anything else has.
                if reason != StopReason::ToolUse {
                    break;
                }
            }
            HarnessEvent::ToolCallStarted { name, .. } => eprintln!("· {name}"),
            HarnessEvent::Error { message, .. } => {
                error = Some(message);
                break;
            }
            // The backstop, for a turn that ends without a final assistant entry. Ignored
            // before the first response, because the session is idle when the prompt arrives.
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            } if started => break,
            _ => {}
        }
    }

    let _ = writer.write(&UiCommand::Detach).await;
    Ok(Outcome {
        text,
        stop_reason,
        error,
    })
}

/// An attach position past every entry there could be.
///
/// The snapshot carries everything up to the cursor and the replay carries everything after,
/// so asking from the far end is how a client says "history is already accounted for, send me
/// only what happens next".
const FROM_END: Cursor = Cursor(u64::MAX);

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(stop_reason: Option<StopReason>, error: Option<&str>) -> Outcome {
        Outcome {
            text: String::new(),
            stop_reason,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn a_finished_answer_exits_zero() {
        assert!(!outcome(Some(StopReason::EndTurn), None).failed());
    }

    #[test]
    fn a_failed_turn_exits_non_zero() {
        assert!(outcome(Some(StopReason::Error), None).failed());
    }

    #[test]
    fn an_interrupted_turn_exits_non_zero() {
        // Ctrl-C during a `-p` run did not produce the answer that was asked for.
        assert!(outcome(Some(StopReason::Aborted), None).failed());
    }

    #[test]
    fn an_error_outside_a_turn_exits_non_zero() {
        assert!(outcome(None, Some("the socket went away")).failed());
    }

    #[test]
    fn a_turn_that_never_reported_a_reason_is_not_treated_as_failure() {
        assert!(!outcome(None, None).failed());
    }
}
