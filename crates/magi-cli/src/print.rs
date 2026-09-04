//! `magi -p "…"` — one prompt, one answer, no terminal.
//!
//! The same daemon, the same journal and the same turn loop the UI drives; only the front end
//! is different. That is the point of running it through the socket rather than in-process: a
//! `-p` run leaves a session behind that `magi --resume` picks up, and there is one
//! implementation of the loop rather than two that agree until they stop agreeing.

use anyhow::Result;
use magi_ipc::{FrameReader, FrameWriter};

use magi_proto::{AgentStatus, Cursor, HarnessEvent, StopReason, UiCommand};
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
    let stream = magi_ipc::connect(socket).await?;
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
    // A tool-using turn stops between rounds: the provider says "tool_use", the daemon runs
    // them, and the session is briefly idle before the next round begins. Treating that idle
    // as the end returns whatever the model had said before it reached for a tool, which for
    // most tool-using prompts is nothing at all.
    let mut awaiting_tools = false;

    while let Ok(event) = reader.read::<HarnessEvent>().await {
        match event {
            HarnessEvent::AssistantStarted { .. } => {
                // Only the last message is printed, so each new one replaces what came before:
                // the intermediate messages in a tool-using turn are working, not the answer.
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
            // Nobody is at the keyboard. Answered rather than ignored: the daemon stops the
            // turn on this question and waits for the answer, so a `-p` run that ignored it
            // hung until it was killed -- with the call committed to the journal, `result:
            // null`, and no way to tell from the outside what it was waiting for.
            //
            // Denied rather than allowed, because `-p` is what goes in a pipeline and a run
            // nobody is watching is the wrong place to widen what a tool may do. `magi.allow`
            // is how a person says in advance what an unattended run may do; anything it does
            // not cover is refused here, and the model is told so it can say so.
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
            // The same, for a question a tool asked in its own words. Answered rather than
            // ignored, and for the identical reason: a `-p` run that left one unanswered would
            // sit there until the question timed itself out, with nothing on screen saying what
            // it was waiting for.
            //
            // The *last* option, by convention, because a tool lists what it is asking for
            // first and the way out last — `once`, `always`, then `no`. There is no better
            // guess available here: nobody is attached, and choosing the first would be
            // choosing the most permissive thing on somebody's behalf.
            HarnessEvent::Asked {
                id,
                tool,
                question,
                options,
                ..
            } => {
                let Some(last) = options.last() else {
                    // A question with no answers cannot be answered. The tool will give up on
                    // its own, and saying so is better than a silent wait.
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
            // The backstop, for a turn that ends without a final assistant entry. Ignored
            // before the first response, because the session is idle when the prompt arrives.
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
