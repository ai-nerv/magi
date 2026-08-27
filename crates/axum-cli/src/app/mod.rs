//! UI state, and the reduction of harness events onto it.
//!
//! Pure: no terminal, no socket. The driver feeds it events and keys and asks it what to
//! draw, which is what lets the whole state machine be tested without a pty.

use axum_proto::{AgentStatus, Cursor, Entry, HarnessEvent, MessageId, ToolCallId};
use axum_tui::Editor;
use axum_tui::scrollback::Scrollback;

/// Whether the transcript above the live region is up to date.
#[derive(Debug, PartialEq, Eq)]
pub enum Flush {
    /// Nothing new has settled.
    Nothing,
    /// Entries `..n` are final and belong in scrollback.
    Upto(usize),
}

/// Add two token counts.
///
/// Written out because `Usage` is four independent counters, and summing three while
/// forgetting the fourth shows up as a footer that quietly reads low.
fn add(total: axum_proto::Usage, next: axum_proto::Usage) -> axum_proto::Usage {
    axum_proto::Usage {
        input: total.input + next.input,
        output: total.output + next.output,
        cache_read: total.cache_read + next.cache_read,
        cache_write: total.cache_write + next.cache_write,
    }
}

/// Everything the UI knows.
pub struct App {
    /// Transcript in order. Entries before `flushed` are already in scrollback.
    entries: Vec<Entry>,
    /// How many entries have been handed to the terminal's scrollback.
    flushed: usize,
    /// Highest cursor seen, so a reconnect resumes rather than replays.
    cursor: Cursor,
    /// What the agent is doing.
    status: AgentStatus,
    /// The prompt buffer.
    pub editor: Editor,
    /// The transcript we own, used by the alt-screen backend.
    ///
    /// Inline mode leaves history to the terminal and never touches this; alt mode has no
    /// terminal history to leave it to.
    pub scrollback: Scrollback,
    /// The open completion popup, if any.
    pub completion: Option<axum_tui::complete::Completion>,
    /// Whether the socket is currently up.
    pub connected: bool,
    /// Spinner phase.
    pub tick: usize,
    /// Which model is answering, as the daemon reported it.
    ///
    /// From the daemon rather than read from the configuration here: a UI reading the config
    /// for itself would name whatever is configured *now*, which after an edit is not what the
    /// daemon on the other end of the socket is actually talking to.
    pub model: Option<axum_proto::ModelInfo>,
    /// Everything the daemon says this session could switch to.
    pub choices: Vec<axum_proto::ModelChoice>,
    /// The open selection list, if any.
    pub picker: Option<axum_tui::picker::Picker>,
    /// How much of each tool result to show.
    pub detail: axum_tui::transcript::Detail,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// A UI with an empty transcript.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            flushed: 0,
            cursor: Cursor::ZERO,
            status: AgentStatus::Idle,
            editor: Editor::new(),
            scrollback: Scrollback::new(),
            completion: None,
            connected: false,
            model: None,
            choices: Vec::new(),
            picker: None,
            detail: axum_tui::transcript::Detail::Preview,
            tick: 0,
        }
    }

    /// The transcript.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every token this session has spent.
    ///
    /// Derived from the transcript rather than accumulated as events arrive. A running total
    /// has to be right in two places — the snapshot on attach, and each event after it — and a
    /// reattach replays events the snapshot already counted, so the two disagree by however
    /// much was replayed. Folding the entries cannot double count, because there is only one
    /// of each.
    #[must_use]
    pub fn usage(&self) -> axum_proto::Usage {
        self.entries
            .iter()
            .fold(axum_proto::Usage::default(), |total, entry| match entry {
                Entry::Assistant { usage, .. } => add(total, *usage),
                _ => total,
            })
    }

    /// Tokens the most recent request actually sent.
    ///
    /// How full the window is, which is not the same question as what the session has spent:
    /// the window holds one conversation, and an afternoon that used ten windows' worth is not
    /// ten times full. Zero until a turn has reported any, and after a compaction it drops —
    /// which is the point of compacting.
    #[must_use]
    pub fn last_prompt_tokens(&self) -> u64 {
        self.entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                Entry::Assistant { usage, .. } if usage.prompt_tokens() > 0 => {
                    Some(usage.prompt_tokens())
                }
                _ => None,
            })
            .unwrap_or(0)
    }

    /// Entries that have not yet reached scrollback.
    #[must_use]
    pub fn live(&self) -> &[Entry] {
        &self.entries[self.flushed..]
    }

    /// Position to resume from after a disconnect.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// What the agent is doing.
    #[must_use]
    pub fn status(&self) -> &AgentStatus {
        &self.status
    }

    /// Whether a turn is in flight, which is what gates Enter and enables Esc.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        !matches!(self.status, AgentStatus::Idle)
    }

    /// Fold one harness event into the state.
    pub fn apply(&mut self, event: HarnessEvent) {
        self.cursor = self.cursor.max(event.cursor());
        match event {
            HarnessEvent::SessionSnapshot {
                cursor,
                entries,
                status,
                model,
                choices,
                ..
            } => {
                // A snapshot describes the session as of the cursor the UI asked to resume
                // from. On a cold attach that is nothing; on a reattach it is history this UI
                // has already written to scrollback, and re-flushing it prints the transcript
                // again on every reconnect.
                //
                // All but the last entry are therefore already on screen. The last is kept
                // live because it may still be streaming, and a scrollback line cannot be
                // taken back once written.
                let resuming = cursor > Cursor::ZERO;
                self.flushed = if resuming {
                    entries.len().saturating_sub(1)
                } else {
                    0
                };
                let unconfigured = model.is_none();
                self.model = model;
                self.choices = choices;
                let empty = entries.is_empty();
                self.entries = entries;
                self.status = status;
                // Said once, on a session that has not started yet. A fresh install points at
                // a model whose key nobody has set, and the whole of what it told you was
                // `no-model` in a corner of the footer — true, and no help at all.
                if unconfigured && empty && !self.choices.is_empty() {
                    self.show_notice(
                        "No model is configured. Type `/model` to choose one.".to_owned(),
                    );
                }
            }
            HarnessEvent::UserMessage { id, text, .. } => {
                self.entries.push(Entry::User { id, text });
            }
            HarnessEvent::AssistantStarted { id, .. } => {
                self.entries.push(Entry::Assistant {
                    id,
                    text: String::new(),
                    thinking: String::new(),
                    stop_reason: None,
                    error: None,
                    signatures: axum_proto::Signatures::default(),
                    usage: axum_proto::Usage::default(),
                });
            }
            HarnessEvent::AssistantDelta {
                id, text, thinking, ..
            } => {
                if let Some(Entry::Assistant {
                    text: body,
                    thinking: reasoning,
                    ..
                }) = self.assistant_mut(&id)
                {
                    body.push_str(&text);
                    reasoning.push_str(&thinking);
                }
            }
            HarnessEvent::AssistantEnded {
                id,
                stop_reason,
                error,
                usage,
                ..
            } => {
                if let Some(Entry::Assistant {
                    stop_reason: stop,
                    error: err,
                    usage: cost,
                    ..
                }) = self.assistant_mut(&id)
                {
                    *stop = Some(stop_reason);
                    *err = error;
                    *cost = usage;
                }
            }
            HarnessEvent::ModelChanged { model, .. } => self.model = model,
            // Not a transcript entry: the request was understood and declined, which is a
            // fact about what the UI asked rather than about the conversation.
            HarnessEvent::Refused { message, .. } => self.show_notice(message),
            HarnessEvent::Branched { id, keeps, .. } => {
                self.entries.push(Entry::Branch { id, keeps });
            }
            HarnessEvent::Compacted {
                id,
                summary,
                replaces,
                ..
            } => self.entries.push(Entry::Compaction {
                id,
                summary,
                replaces,
            }),
            HarnessEvent::ToolCallStarted { id, name, args, .. } => {
                self.entries.push(Entry::Tool {
                    id,
                    name,
                    args,
                    result: None,
                    thought_signature: None,
                });
            }
            HarnessEvent::ToolCallEnded { id, result, .. } => {
                if let Some(Entry::Tool { result: slot, .. }) = self.tool_mut(&id) {
                    *slot = Some(result);
                }
            }
            HarnessEvent::StatusChanged { status, .. } => self.status = status,
            HarnessEvent::Error { class, message, .. } => {
                self.entries.push(Entry::Assistant {
                    id: MessageId::new("error"),
                    text: String::new(),
                    thinking: String::new(),
                    stop_reason: Some(axum_proto::StopReason::Error),
                    error: Some(format!("{class:?}: {message}")),
                    signatures: axum_proto::Signatures::default(),
                    usage: axum_proto::Usage::default(),
                });
            }
        }
    }

    /// Which entries have settled and can be written to scrollback.
    ///
    /// An entry settles when a later one exists: the last entry may still be streaming, and a
    /// line written to scrollback cannot be taken back.
    #[must_use]
    pub fn settled(&self) -> Flush {
        let settled = self.entries.len().saturating_sub(1);
        if settled > self.flushed {
            Flush::Upto(settled)
        } else {
            Flush::Nothing
        }
    }

    /// Record that entries up to `n` reached scrollback.
    pub fn mark_flushed(&mut self, n: usize) {
        self.flushed = n.min(self.entries.len());
    }

    /// Drop the transcript without touching the daemon.
    ///
    /// `/clear` hides history from the view; it does not delete it. The journal is
    /// append-only, and a UI command must never be able to rewrite it.
    pub fn clear_view(&mut self) {
        self.entries.clear();
        self.flushed = 0;
    }

    /// Append a local notice to the transcript.
    ///
    /// Notices are UI-side only and never reach the journal: `/help` output is not something
    /// a future session should replay, and the daemon never authored it.
    pub fn show_notice(&mut self, text: String) {
        self.entries.push(Entry::Notice { text });
    }

    /// Append the keybinding reference.
    pub fn show_help(&mut self) {
        self.show_notice(crate::help::text());
    }

    /// Open the model list.
    ///
    /// Every model, not only the reachable ones: somebody asking this question has usually
    /// configured nothing, and a list narrowed to what already works would be empty exactly
    /// when they most need it to name a variable.
    pub fn open_model_picker(&mut self) {
        let choices = self
            .choices
            .iter()
            .map(|choice| axum_tui::picker::Choice {
                value: choice.name.clone(),
                detail: if choice.requirement.is_empty() {
                    axum_tui::footer::format_tokens(choice.context_window)
                } else {
                    choice.requirement.clone()
                },
                ready: choice.requirement.is_empty(),
            })
            .collect();
        let current = self.model.as_ref().map(|m| m.name.clone());
        let picker = axum_tui::picker::Picker::new("Model", choices, current.as_deref());
        if picker.offers_nothing() {
            self.show_notice(
                "No providers are declared. `axum models --all` lists what axum ships.".to_owned(),
            );
            return;
        }
        self.picker = Some(picker);
    }

    /// Show every line of each tool result, or go back to the preview.
    ///
    /// A whole transcript at a time rather than one block: picking a block needs a selection,
    /// and a selection needs keys, a highlight and a rule for what happens when the thing
    /// selected scrolls away. The question being asked is almost always about the last result.
    pub fn toggle_detail(&mut self) -> axum_tui::transcript::Detail {
        self.detail = match self.detail {
            axum_tui::transcript::Detail::Preview => axum_tui::transcript::Detail::Full,
            axum_tui::transcript::Detail::Full => axum_tui::transcript::Detail::Preview,
        };
        self.detail
    }

    /// Recompute the completion popup from the current prompt.
    pub fn refresh_completion(&mut self, list_paths: &dyn Fn(&str) -> Vec<String>) {
        let (row, col) = self.editor.cursor();
        let line = self.editor.lines()[row].clone();
        self.completion = axum_tui::complete::resolve(&line, col, list_paths);
    }

    fn assistant_mut(&mut self, id: &MessageId) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Assistant { id: candidate, .. } if candidate == id))
    }

    fn tool_mut(&mut self, id: &ToolCallId) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Tool { id: candidate, .. } if candidate == id))
    }
}

#[cfg(test)]
mod tests;
