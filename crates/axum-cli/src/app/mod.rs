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

/// What an open selection list is choosing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picking {
    /// Which model answers.
    Model,
    /// How much reasoning to ask for.
    Thinking,
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
    /// When the current turn started, for the elapsed clock.
    working_since: Option<std::time::Instant>,
    /// A notice to show once the session's own entries have arrived.
    ///
    /// Not shown immediately: attaching replaces `entries` wholesale with what the daemon has,
    /// so anything appended before the first snapshot is discarded by it. This is for things
    /// the UI knows at startup and the daemon does not.
    pending_notice: Option<String>,
    /// Commands submitted but not yet handed to a daemon.
    ///
    /// Set by the driver from the command channel: a prompt sent while the daemon is away
    /// waits in it rather than being lost, and an emptied prompt box with nothing on screen
    /// gave no way to tell those two apart.
    pub queued: usize,
    /// Spinner phase.
    pub tick: usize,
    /// Which model is answering, as the daemon reported it.
    ///
    /// From the daemon rather than read from the configuration here: a UI reading the config
    /// for itself would name whatever is configured *now*, which after an edit is not what the
    /// daemon on the other end of the socket is actually talking to.
    pub model: Option<axum_proto::ModelInfo>,
    /// How much reasoning is being asked for.
    pub thinking: String,
    /// Whether the model answering can reason at all.
    model_reasons: bool,
    /// Everything the daemon says this session could switch to.
    pub choices: Vec<axum_proto::ModelChoice>,
    /// The open selection list, if any.
    pub picker: Option<axum_tui::picker::Picker>,
    /// What that list is choosing.
    ///
    /// Held beside the list rather than inside it, because the list is a generic widget and
    /// this is the one thing about it only its opener knows. Without it every list's answer
    /// went to the same place, and picking a thinking level asked for a model called "medium".
    pub picking: Option<Picking>,
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
            thinking: "off".to_owned(),
            model_reasons: false,
            choices: Vec::new(),
            picker: None,
            picking: None,
            detail: axum_tui::transcript::Detail::Preview,
            pending_notice: None,
            queued: 0,
            working_since: None,
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

    /// Record what the agent is doing, and when it started doing it.
    ///
    /// The clock is kept here rather than derived from the spinner tick: the tick runs
    /// whether or not a turn is in flight, so it says how long the UI has been open and not
    /// how long you have been waiting.
    fn set_status(&mut self, status: AgentStatus) {
        let was_idle = matches!(self.status, AgentStatus::Idle);
        let now_idle = matches!(status, AgentStatus::Idle);
        if now_idle {
            self.working_since = None;
        } else if was_idle {
            self.working_since = Some(std::time::Instant::now());
        }
        self.status = status;
    }

    /// How long the current turn has been running.
    #[must_use]
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.working_since.map(|t| t.elapsed())
    }

    /// Whether this session has said anything yet.
    ///
    /// Notices do not count. A fresh install opens with "no model is configured", which is a
    /// message from axum about itself rather than the beginning of a conversation -- and
    /// treating it as one replaced the whole first screen with a single line.
    #[must_use]
    pub fn started(&self) -> bool {
        self.entries
            .iter()
            .any(|e| !matches!(e, Entry::Notice { .. }))
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
                thinking,
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
                self.model_reasons = self.model.as_ref().is_some_and(|chosen| {
                    choices.iter().any(|c| c.name == chosen.name && c.reasoning)
                });
                if !thinking.is_empty() {
                    self.thinking = thinking;
                }
                self.choices = choices;
                let empty = entries.is_empty();
                self.entries = entries;
                self.set_status(status);
                // Now that the daemon's entries have replaced ours, anything the UI knew at
                // startup can be added without the snapshot eating it.
                if let Some(text) = self.pending_notice.take() {
                    self.show_notice(text);
                }
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
            // Said in the transcript, once the conversation has started. Which model answered
            // is part of the record, and a switch that changes only two dim words in the
            // footer leaves no mark on the place a reader actually reads.
            HarnessEvent::ModelChanged { model, .. } => {
                let before = self.model.as_ref().map(|m| m.name.clone());
                let after = model.as_ref().map(|m| m.name.clone());
                if self.started() && before != after {
                    if let Some(name) = after {
                        self.show_notice(format!("Model is now `{name}`."));
                    }
                }
                self.model = model;
            }
            // Not a transcript entry: the request was understood and declined, which is a
            // fact about what the UI asked rather than about the conversation.
            HarnessEvent::Refused { message, .. } => self.show_notice(message),
            // A rule marks the boundary between what is still sent and what is not. On a view
            // with nothing above it there is no boundary to mark, only a line saying nothing
            // is sent from here -- which is every empty session.
            HarnessEvent::Branched { id, keeps, .. } => {
                if self.started() {
                    self.entries.push(Entry::Branch { id, keeps });
                }
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
            HarnessEvent::StatusChanged { status, .. } => self.set_status(status),
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

    /// Hold a notice until the first snapshot has landed.
    pub fn notice_after_attach(&mut self, text: String) {
        self.pending_notice = Some(text);
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
        let mut daemon_is_behind = false;
        let choices: Vec<axum_tui::picker::Choice> = self
            .choices
            .iter()
            .map(|choice| {
                // The daemon says "set OPENROUTER_API_KEY". That is a lie to somebody who has
                // already set it — a daemon captures its environment when it starts and
                // outlives the shell that started it, so a key exported afterwards never
                // reaches it. Only this process can tell the two apart, because only this
                // process is the one you just typed in.
                let stale = !choice.wants_vars.is_empty()
                    && choice
                        .wants_vars
                        .iter()
                        .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()));
                if stale {
                    daemon_is_behind = true;
                }
                axum_tui::picker::Choice {
                    value: choice.name.clone(),
                    detail: if choice.requirement.is_empty() {
                        axum_tui::footer::format_tokens(choice.context_window)
                    } else if stale {
                        "you have this key — the daemon predates it".to_owned()
                    } else {
                        choice.requirement.clone()
                    },
                    ready: choice.requirement.is_empty(),
                }
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
        if daemon_is_behind {
            self.show_notice(
                "A key you have set is not visible to the daemon serving this directory — it was \
                 started before you set it. Run `axum stop`, then start again."
                    .to_owned(),
            );
        }
        self.picker = Some(picker);
        self.picking = Some(Picking::Model);
    }

    /// Open the reasoning-level list.
    ///
    /// Every level, marked with what this model can actually do: a level the catalog says it
    /// refuses is shown and cannot be taken, for the same reason an unconfigured provider is.
    pub fn open_thinking_picker(&mut self) {
        const LEVELS: [(&str, &str); 6] = [
            ("off", "no reasoning — the default"),
            ("minimal", "the smallest budget the model offers"),
            ("low", "a small budget"),
            ("medium", "the usual budget"),
            ("high", "a large budget"),
            ("max", "the largest budget the model offers"),
        ];
        let reasons = self.model_reasons;
        let choices = LEVELS
            .iter()
            .map(|(value, detail)| axum_tui::picker::Choice {
                value: (*value).to_owned(),
                detail: if reasons || *value == "off" {
                    (*detail).to_owned()
                } else {
                    "this model does not reason".to_owned()
                },
                ready: reasons || *value == "off",
            })
            .collect();
        self.picker = Some(axum_tui::picker::Picker::new(
            "Thinking",
            choices,
            Some(self.thinking.as_str()),
        ));
        self.picking = Some(Picking::Thinking);
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
