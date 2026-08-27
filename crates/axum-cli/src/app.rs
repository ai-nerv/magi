//! UI state, and the reduction of harness events onto it.
//!
//! Pure: no terminal, no socket. The driver feeds it events and keys and asks it what to
//! draw, which is what lets the whole state machine be tested without a pty.

use axum_proto::{AgentStatus, Cursor, Entry, HarnessEvent, MessageId, ToolCallId};
use axum_tui::Editor;
use axum_tui::scrollback::Scrollback;

/// What `/help` prints.
const HELP: &str = "\
**Keys**

- `enter` submit — `shift+enter` newline
- `esc` interrupt a running turn
- `tab` accept a completion — `↑/↓` move through it
- `ctrl+x` edit the prompt in `$EDITOR`
- `ctrl+c` clear the prompt, again to quit — `ctrl+d` quit
- `ctrl+a/e` line start/end — `ctrl+k/u` kill — `ctrl+y` yank
- `alt+←/→` word motion — `↑/↓` prompt history

**Commands**

- `/help` this list
- `/clear` clear the view — the journal is untouched
- `/quit` exit

Type `@` to complete a path.";

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
                self.model = model;
                self.entries = entries;
                self.status = status;
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
        self.entries.push(Entry::Assistant {
            id: MessageId::new("notice"),
            text,
            thinking: String::new(),
            stop_reason: Some(axum_proto::StopReason::EndTurn),
            error: None,
            signatures: axum_proto::Signatures::default(),
            usage: axum_proto::Usage::default(),
        });
    }

    /// Append the keybinding reference.
    pub fn show_help(&mut self) {
        self.show_notice(HELP.to_owned());
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
mod tests {
    use super::*;
    use axum_proto::{StopReason, ToolResult};

    fn app_with(events: Vec<HarnessEvent>) -> App {
        let mut app = App::new();
        for event in events {
            app.apply(event);
        }
        app
    }

    #[test]
    fn deltas_accumulate_onto_the_started_message() {
        let app = app_with(vec![
            HarnessEvent::AssistantStarted {
                cursor: Cursor(1),
                id: MessageId::new("a1"),
            },
            HarnessEvent::AssistantDelta {
                cursor: Cursor(2),
                id: MessageId::new("a1"),
                text: "hel".into(),
                thinking: String::new(),
            },
            HarnessEvent::AssistantDelta {
                cursor: Cursor(3),
                id: MessageId::new("a1"),
                text: "lo".into(),
                thinking: String::new(),
            },
        ]);
        match &app.entries()[0] {
            Entry::Assistant { text, .. } => assert_eq!(text, "hello"),
            other => panic!("expected an assistant entry, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_result_lands_on_its_call() {
        let app = app_with(vec![
            HarnessEvent::ToolCallStarted {
                cursor: Cursor(1),
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: "{}".into(),
            },
            HarnessEvent::ToolCallEnded {
                cursor: Cursor(2),
                id: ToolCallId::new("t1"),
                result: ToolResult {
                    output: "ok".into(),
                    is_error: false,
                },
            },
        ]);
        match &app.entries()[0] {
            Entry::Tool { result, .. } => {
                assert_eq!(result.as_ref().map(|r| r.output.as_str()), Some("ok"));
            }
            other => panic!("expected a tool entry, got {other:?}"),
        }
    }

    #[test]
    fn the_cursor_tracks_the_highest_event_seen() {
        let app = app_with(vec![
            HarnessEvent::StatusChanged {
                cursor: Cursor(5),
                status: AgentStatus::Idle,
            },
            HarnessEvent::StatusChanged {
                cursor: Cursor(9),
                status: AgentStatus::Idle,
            },
        ]);
        assert_eq!(app.cursor(), Cursor(9));
    }

    #[test]
    fn a_reordered_event_cannot_rewind_the_cursor() {
        let app = app_with(vec![
            HarnessEvent::StatusChanged {
                cursor: Cursor(9),
                status: AgentStatus::Idle,
            },
            HarnessEvent::StatusChanged {
                cursor: Cursor(2),
                status: AgentStatus::Idle,
            },
        ]);
        assert_eq!(app.cursor(), Cursor(9));
    }

    #[test]
    fn the_last_entry_never_settles_because_it_may_still_stream() {
        let app = app_with(vec![HarnessEvent::UserMessage {
            cursor: Cursor(1),
            id: MessageId::new("m1"),
            text: "hi".into(),
        }]);
        assert_eq!(app.settled(), Flush::Nothing);
    }

    #[test]
    fn earlier_entries_settle_once_a_later_one_arrives() {
        let mut app = app_with(vec![
            HarnessEvent::UserMessage {
                cursor: Cursor(1),
                id: MessageId::new("m1"),
                text: "hi".into(),
            },
            HarnessEvent::AssistantStarted {
                cursor: Cursor(2),
                id: MessageId::new("a1"),
            },
        ]);
        assert_eq!(app.settled(), Flush::Upto(1));
        app.mark_flushed(1);
        assert_eq!(app.settled(), Flush::Nothing);
        assert_eq!(app.live().len(), 1);
    }

    #[test]
    fn a_reattach_snapshot_does_not_reprint_what_is_already_on_screen() {
        let mut app = App::new();
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor(9),
            session: axum_proto::SessionId::new("s"),
            entries: vec![
                Entry::User {
                    id: MessageId::new("m1"),
                    text: "hi".into(),
                },
                Entry::Assistant {
                    id: MessageId::new("a1"),
                    text: "partial".into(),
                    thinking: String::new(),
                    stop_reason: None,
                    error: None,
                    signatures: axum_proto::Signatures::default(),
                    usage: axum_proto::Usage::default(),
                },
            ],
            status: AgentStatus::Idle,
            model: None,
        });
        assert_eq!(
            app.live().len(),
            1,
            "only the in-flight entry is redrawn; the rest is already in scrollback"
        );
        assert_eq!(app.settled(), Flush::Nothing, "nothing new to write");
    }

    #[test]
    fn a_cold_snapshot_still_renders_everything() {
        let mut app = App::new();
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor::ZERO,
            session: axum_proto::SessionId::new("s"),
            entries: vec![Entry::User {
                id: MessageId::new("m9"),
                text: "restored".into(),
            }],
            status: AgentStatus::Idle,
            model: None,
        });
        assert_eq!(app.live().len(), 1, "a cold snapshot is entirely unflushed");
    }

    #[test]
    fn clearing_the_view_empties_the_transcript_and_the_flush_mark() {
        let mut app = app_with(vec![
            HarnessEvent::UserMessage {
                cursor: Cursor(1),
                id: MessageId::new("m1"),
                text: "hi".into(),
            },
            HarnessEvent::AssistantStarted {
                cursor: Cursor(2),
                id: MessageId::new("a1"),
            },
        ]);
        app.mark_flushed(1);
        app.clear_view();
        assert!(app.entries().is_empty());
        assert!(app.live().is_empty());
        assert_eq!(
            app.cursor(),
            Cursor(2),
            "clearing the view keeps the position"
        );
    }

    #[test]
    fn the_popup_follows_the_prompt() {
        let mut app = App::new();
        let none = |_: &str| Vec::new();
        app.editor.insert_str("/qu");
        app.refresh_completion(&none);
        assert!(app.completion.is_some());
        app.editor.clear();
        app.editor.insert_str("plain text");
        app.refresh_completion(&none);
        assert!(app.completion.is_none());
    }

    #[test]
    fn busy_tracks_status() {
        let mut app = App::new();
        assert!(!app.is_busy());
        app.apply(HarnessEvent::StatusChanged {
            cursor: Cursor(1),
            status: AgentStatus::Working {
                label: "Thinking".into(),
            },
        });
        assert!(app.is_busy());
    }

    #[test]
    fn an_error_event_becomes_a_visible_transcript_entry() {
        let app = app_with(vec![HarnessEvent::Error {
            cursor: Cursor(1),
            class: axum_proto::ErrorClass::Overload,
            message: "busy".into(),
        }]);
        match &app.entries()[0] {
            Entry::Assistant {
                stop_reason, error, ..
            } => {
                assert_eq!(*stop_reason, Some(StopReason::Error));
                assert!(error.as_deref().unwrap_or_default().contains("busy"));
            }
            other => panic!("expected an assistant entry, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use axum_proto::{ModelInfo, SessionId, StopReason, Usage};

    fn ended(id: &str, cursor: u64, usage: Usage) -> HarnessEvent {
        HarnessEvent::AssistantEnded {
            cursor: Cursor(cursor),
            id: MessageId::new(id),
            stop_reason: StopReason::EndTurn,
            error: None,
            usage,
        }
    }

    fn spent(input: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
        }
    }

    #[test]
    fn a_turn_reports_what_it_cost() {
        let mut app = App::new();
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(1),
            id: MessageId::new("a1"),
        });
        app.apply(ended("a1", 1, spent(100, 20)));
        assert_eq!(app.usage().input, 100);
        assert_eq!(app.usage().output, 20);
    }

    #[test]
    fn a_reattach_does_not_count_the_same_turn_twice() {
        // The reason this is derived rather than accumulated. A snapshot carries entries that
        // already include their cost, and the replay after it re-sends the events that
        // produced them: a running total adds both and reads high by however much was replayed.
        let mut app = App::new();
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(1),
            id: MessageId::new("a1"),
        });
        app.apply(ended("a1", 1, spent(100, 20)));
        let entries = app.entries().to_vec();

        let mut rejoined = App::new();
        rejoined.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor(1),
            session: SessionId::new("s"),
            entries,
            status: AgentStatus::Idle,
            model: None,
        });
        rejoined.apply(ended("a1", 1, spent(100, 20)));
        assert_eq!(rejoined.usage().input, 100, "counted once, not twice");
    }

    #[test]
    fn window_fullness_is_the_last_prompt_not_the_running_total() {
        // An afternoon that spent ten windows' worth is not ten times full.
        let mut app = App::new();
        for (n, cost) in [(1, spent(1000, 10)), (2, spent(1200, 10))] {
            app.apply(HarnessEvent::AssistantStarted {
                cursor: Cursor(n),
                id: MessageId::new(format!("a{n}")),
            });
            app.apply(ended(&format!("a{n}"), n, cost));
        }
        assert_eq!(app.usage().input, 2200, "the session spent both");
        assert_eq!(app.last_prompt_tokens(), 1200, "the window holds the last");
    }

    #[test]
    fn a_turn_that_reported_nothing_does_not_reset_the_gauge() {
        // A refusal costs nothing and is journalled with a zero. Reading it as "the window is
        // empty now" would make the gauge flicker to zero on every error.
        let mut app = App::new();
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(1),
            id: MessageId::new("a1"),
        });
        app.apply(ended("a1", 1, spent(900, 10)));
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(2),
            id: MessageId::new("a2"),
        });
        app.apply(ended("a2", 2, Usage::default()));
        assert_eq!(app.last_prompt_tokens(), 900);
    }

    #[test]
    fn the_model_comes_from_the_daemon() {
        let mut app = App::new();
        assert!(app.model.is_none(), "nothing is assumed before it says");
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor::ZERO,
            session: SessionId::new("s"),
            entries: Vec::new(),
            status: AgentStatus::Idle,
            model: Some(ModelInfo {
                name: "p/m".into(),
                context_window: 1000,
            }),
        });
        assert_eq!(app.model.expect("a model").name, "p/m");
    }
}
