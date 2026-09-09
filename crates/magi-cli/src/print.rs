//! `magi -p "…"` — one prompt, one answer, no terminal. Run through the socket like the UI, so a
//! `-p` run leaves a session `magi --resume` picks up and there is one implementation of the loop.

use anyhow::Result;
use magi_ipc::{FrameReader, FrameWriter};

use magi_proto::{AgentStatus, Cursor, HarnessEvent, StopReason, UiCommand};
use std::path::Path;

/// What a finished print run reports to the shell. An error is an exit code, not a panic: `-p` goes
/// in a pipeline, and a caller should not have to parse the answer to know it failed.
pub struct Outcome {
    pub text: String,
    pub stop_reason: Option<StopReason>,
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
    let stream = magi_ipc::connect(socket).await?;
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    // Attached from the end rather than from zero: a resumed session's history is context for the
    // model, not output for this run.
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: FROM_END,
            // No terminal and nobody watching, so no rows are reserved for a surface.
            draws: false,
        })
        .await?;
    if !matches!(
        reader.read::<HarnessEvent>().await?,
        HarnessEvent::SessionSnapshot { .. }
    ) {
        anyhow::bail!("the session did not open with a snapshot");
    }
    writer
        .write(&UiCommand::SubmitPrompt {
            text: prompt,
            aside: String::new(),
        })
        .await?;

    let mut text = String::new();
    let mut stop_reason = None;
    let mut error = None;
    let mut started = false;
    // A tool-using turn stops between rounds, so treating that idle as the end would return
    // whatever the model had said before it reached for a tool.
    let mut awaiting_tools = false;

    while let Ok(event) = reader.read::<HarnessEvent>().await {
        match event {
            HarnessEvent::AssistantStarted { .. } => {
                // Only the last message is printed; the intermediate ones are working, not answer.
                text.clear();
                started = true;
                awaiting_tools = false;
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
                awaiting_tools = true;
            }
            HarnessEvent::ToolCallStarted { name, .. } => eprintln!("· {name}"),
            // Nobody is at the keyboard, and the daemon waits for an answer, so a `-p` run that
            // ignored this would hang. Denied rather than allowed: `magi.allow` is how a person
            // says in advance what an unattended run may do, and anything else is refused here.
            HarnessEvent::PermissionAsked {
                id, tool, action, ..
            } => {
                eprintln!(
                    "· {tool} was not permitted to {} {} -- nothing is attached to ask, and \
                     `magi.allow` does not cover it",
                    action.verb(),
                    action.subject()
                );
                writer
                    .write(&UiCommand::Permit {
                        id,
                        decision: magi_proto::permit::Decision::Deny,
                    })
                    .await?;
            }
            // The same, for a question a tool asked in its own words: a `-p` run that left one
            // unanswered would sit until the question timed out. The *last* option by convention,
            // because a tool lists what it wants first and the way out last.
            HarnessEvent::Asked {
                id,
                tool,
                question,
                options,
                ..
            } => {
                let Some(last) = options.last() else {
                    // A question with no answers cannot be answered; the tool gives up on its own.
                    eprintln!("· {tool} asked \"{question}\" and offered nothing to answer with");
                    continue;
                };
                eprintln!(
                    "· {tool} asked \"{question}\" -- nothing is attached to answer, so `{}` \
                     was taken",
                    last.label
                );
                writer
                    .write(&UiCommand::Answered {
                        id,
                        choice: last.id.clone(),
                    })
                    .await?;
            }
            HarnessEvent::Error { message, .. } => {
                error = Some(message);
                break;
            }
            // The backstop, for a turn that ends without a final assistant entry.
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            } if started && !awaiting_tools => break,
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

/// An attach position past every entry there could be: the snapshot carries everything up to the
/// cursor, so asking from the far end says "send me only what happens next".
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
