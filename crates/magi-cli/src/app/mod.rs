//! UI state, and the reduction of harness events onto it. Pure: no terminal, no socket, so the
//! whole state machine can be tested without a pty.

use magi_proto::{AgentStatus, Cursor, Entry, HarnessEvent, MessageId, ToolCallId};
use magi_tui::Editor;
use magi_tui::overlay::Overlay;
use magi_tui::scrollback::Scrollback;

/// Add two token counts. `Usage` is four independent counters; missing one reads low in the footer.
fn add(total: magi_proto::Usage, next: magi_proto::Usage) -> magi_proto::Usage {
    magi_proto::Usage {
        input: total.input + next.input,
        output: total.output + next.output,
        cache_read: total.cache_read + next.cache_read,
        cache_write: total.cache_write + next.cache_write,
    }
}

/// Everything the UI knows.
pub struct App {
    entries: Vec<Entry>,
    /// Highest cursor seen, so a reconnect resumes rather than replays.
    cursor: Cursor,
    status: AgentStatus,
    pub editor: Editor,
    /// The transcript, which magi owns: the alternate screen has no terminal history to defer to.
    pub scrollback: Scrollback,

    pub connected: bool,
    working_since: Option<std::time::Instant>,
    /// Held back until the first snapshot: attaching replaces `entries` wholesale.
    pending_notice: Option<String>,
    /// Overrides the fixed "nothing is configured" sentence when only the provider key is unset.
    pub no_model: Option<String>,
    /// A scope's label is written in terms of the action, so turning one back needs that action.
    pub asking_about: magi_proto::permit::Action,
    /// Submitted but not yet handed to a daemon: a prompt sent while it is away waits here.
    pub queued: usize,
    pub tick: usize,
    /// In hundredths of a tick, so `scan_speed = 0.5` is half as fast rather than stopped.
    scan_phase: usize,
    /// As the daemon reported it, not read from the config here — after an edit the two differ.
    pub model: Option<magi_proto::ModelInfo>,
    pub thinking: String,
    model_reasons: bool,
    pub choices: Vec<magi_proto::ModelChoice>,
    /// A list or a completion popup. One slot: running a command closes the popup that offered it.
    pub overlay: Option<Overlay>,
    pub pane: Option<magi_tui::pane::Pane>,
    /// Recorded from the start whether or not anybody looks, in one bounded ring.
    pub timeline: magi_tui::trace::Trace,
    pub picking: Option<Picking>,
    /// One at a time: a turn runs its tool calls in order.
    pub surface: Option<surfacing::Surfacing>,
    pub detail: magi_tui::transcript::Detail,
    /// `project/role/id` when melchior is running, which does the naming; the project alone otherwise.
    pub named: String,
    pub tease: magi_tui::tease::Tease,
    pub landing: magi_tui::decrypt::Landing,
    pub trace: magi_tui::beacon::Trace,
    pub modal: crate::keys::Modal,
    /// Pushed by melchior for the `$` popup: a completion offered on a keystroke cannot go look.
    pub reachable: Vec<crate::melchior::Peer>,
    /// Whose session is on screen, `None` for this one's own; see [`crewing`].
    pub attached: Option<crate::melchior::Peer>,
    pub waiting: usize,
    /// What the screen has seen decided, for handing to a session this one takes on as a child.
    pub granted: Vec<magi_proto::permit::Grant>,
    was_blank: bool,
    /// A transcript coordinate rather than a screen one, so scrolling carries the highlight.
    pub hovering: Option<(usize, u16)>,
    /// Kept after the button comes up, so the highlight stays until the next click.
    pub selection: Option<magi_tui::select::Selection>,
    /// Blocks showing the opposite of `detail`, by membership, so the fold key still moves the rest.
    pub flipped: std::collections::BTreeSet<ToolCallId>,
    pub owners: Vec<Option<ToolCallId>>,
    /// Parallel to the scrollback, for every block: an assistant message has no id to key on.
    pub blocks: Vec<Option<usize>>,
    /// Recorded by the drawing pass, which is the only thing that knows where the prompt begins.
    pub live_rows: std::ops::Range<u16>,
    /// `None` when nothing holds rows, so a pointer over a picker is not translated for a closed surface.
    pub surface_rect: Option<ratatui::layout::Rect>,
    /// Recorded by the layout rather than recomputed; `None` when the corner wears nothing.
    pub corner_rect: Option<ratatui::layout::Rect>,
    /// So a press outside the pane can close it. `None` when none is open.
    pub pane_rect: Option<ratatui::layout::Rect>,
    pub corner: magi_tui::corner::Corner,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pane: None,
            timeline: magi_tui::trace::Trace::new(),
            entries: Vec::new(),
            cursor: Cursor::ZERO,
            status: AgentStatus::Idle,
            editor: Editor::new(),
            scrollback: Scrollback::new(),
            connected: false,
            model: None,
            thinking: "off".to_owned(),
            model_reasons: false,
            choices: Vec::new(),
            overlay: None,
            picking: None,
            // Folded; the handle at the foot of each block opens the one you care about.
            detail: magi_tui::transcript::Detail::Preview,
            named: String::new(),
            tease: magi_tui::tease::Tease::new(opener()),
            landing: magi_tui::decrypt::Landing::default(),
            trace: magi_tui::beacon::Trace::default(),
            modal: crate::keys::Modal::default(),
            reachable: Vec::new(),
            attached: None,
            waiting: 0,
            granted: Vec::new(),
            was_blank: true,
            hovering: None,
            selection: None,
            flipped: std::collections::BTreeSet::new(),
            owners: Vec::new(),
            blocks: Vec::new(),
            surface: None,
            live_rows: 0..0,
            surface_rect: None,
            corner_rect: None,
            pane_rect: None,
            corner: magi_tui::corner::Corner::default(),
            pending_notice: None,
            no_model: None,
            asking_about: magi_proto::permit::Action::Read {
                path: String::new(),
            },
            queued: 0,
            working_since: None,
            tick: 0,
            scan_phase: 0,
        }
    }

    /// Move the empty prompt on, and put it back to a fresh opener when it changes emptiness.
    pub fn settle_prompt(&mut self) {
        let blank = self.editor.is_blank();
        if blank != self.was_blank {
            self.tease.restart(opener());
        } else if blank {
            self.tease.advance(magi_tui::glyph::placeholders());
        }
        self.was_blank = blank;
    }

    /// Advance both clocks by one frame.
    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.scan_phase = self
            .scan_phase
            .wrapping_add(usize::from(magi_tui::metric::scan_speed()));
    }

    /// The scan's phase in whole ticks, which is what the border is drawn from.
    #[must_use]
    pub fn scan_tick(&self) -> usize {
        self.scan_phase / usize::from(magi_tui::metric::NORMAL)
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every token this session has spent, folded from the transcript: a running total would double on reattach.
    #[must_use]
    pub fn usage(&self) -> magi_proto::Usage {
        self.entries
            .iter()
            .fold(magi_proto::Usage::default(), |total, entry| match entry {
                Entry::Assistant { usage, .. } => add(total, *usage),
                _ => total,
            })
    }

    /// Tokens the most recent request sent: how full the window is. Zero until reported, and it drops on compaction.
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

    /// Position to resume from after a disconnect.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Record what the agent is doing, and when it started. Its own clock: the spinner runs between turns.
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

    #[must_use]
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.working_since.map(|t| t.elapsed())
    }

    /// Whether this session has said anything yet. Notices do not count — a fresh install opens with one.
    #[must_use]
    pub fn started(&self) -> bool {
        self.entries
            .iter()
            .any(|e| !matches!(e, Entry::Notice { .. }))
    }

    #[must_use]
    pub fn status(&self) -> &AgentStatus {
        &self.status
    }

    /// Whether a turn is in flight, which is what gates Enter and enables Esc.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        !matches!(self.status, AgentStatus::Idle)
    }

    /// Drop the transcript without touching the daemon: the journal is append-only and `/clear` only hides it.
    pub fn clear_view(&mut self) {
        self.entries.clear();
    }

    /// Append a local notice. Notices are UI-side only and never reach the journal.
    pub fn show_notice(&mut self, text: String) {
        self.entries.push(Entry::Notice { text });
    }

    pub fn notice_after_attach(&mut self, text: String) {
        self.pending_notice = Some(text);
    }

    pub fn show_help(&mut self) {
        self.show_notice(crate::help::text());
    }

    /// Open the model list: every model, not only the reachable ones, or it is empty before anything works.
    pub fn open_model_picker(&mut self) {
        let choices: Vec<magi_tui::picker::Choice> = self
            .choices
            .iter()
            .map(|choice| magi_tui::picker::Choice {
                value: choice.name.clone(),
                detail: if choice.requirement.is_empty() {
                    magi_tui::footer::format_tokens(choice.context_window)
                } else {
                    choice.requirement.clone()
                },
                ready: choice.requirement.is_empty(),
            })
            .collect();
        let current = self.model.as_ref().map(|m| m.name.clone());
        let picker = magi_tui::picker::Picker::new("Model", choices, current.as_deref());
        if picker.offers_nothing() {
            self.show_notice(
                "No providers are declared. `magi models --all` lists what magi ships.".to_owned(),
            );
            return;
        }
        self.overlay = Some(picker.into());
        self.picking = Some(Picking::Model);
    }

    /// Open the reasoning-level list. Every level is shown; one the catalog says this model refuses is not takeable.
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
            .map(|(value, detail)| magi_tui::picker::Choice {
                value: (*value).to_owned(),
                detail: if reasons || *value == "off" {
                    (*detail).to_owned()
                } else {
                    "this model does not reason".to_owned()
                },
                ready: reasons || *value == "off",
            })
            .collect();
        self.overlay = Some(
            magi_tui::picker::Picker::new("Thinking", choices, Some(self.thinking.as_str())).into(),
        );
        self.picking = Some(Picking::Thinking);
    }

    /// Show every line of each tool result, or go back to the preview. A whole transcript at a time.
    pub fn toggle_detail(&mut self) -> magi_tui::transcript::Detail {
        self.detail = match self.detail {
            magi_tui::transcript::Detail::Preview => magi_tui::transcript::Detail::Full,
            magi_tui::transcript::Detail::Full => magi_tui::transcript::Detail::Preview,
        };
        self.detail
    }

    /// Recompute the completion popup. The command menu only opens on the command line.
    pub fn refresh_completion(&mut self, list_paths: &dyn Fn(&str) -> Vec<String>) {
        let (row, col) = self.editor.cursor();
        let line = self.editor.lines()[row].clone();
        // `$` offers whoever is listening, read on the keystroke: a dead instance did not deregister.
        let resolved = magi_tui::complete::resolve_with(&line, col, list_paths, &|_| {
            self.reachable.iter().map(|them| them.id.clone()).collect()
        });
        self.overlay = resolved
            .filter(|found| {
                found.kind != magi_tui::complete::Kind::Command || self.modal.commanding()
            })
            .map(Into::into);
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

mod crewing;
mod kin;
pub use crewing::{Seat, drives, spoken};
mod picking;
pub use picking::Picking;
mod applying;
mod asked;
mod folding;
#[cfg(test)]
mod retracting;
mod sessions;
pub mod surfacing;
#[cfg(test)]
mod tests;
#[path = "views.rs"]
mod views;

/// A line for the box to open with, drawn fresh each time the prompt empties.
pub(crate) fn opener() -> &'static str {
    let list = magi_tui::glyph::openers();
    if list.is_empty() {
        return "";
    }
    list.get(magi_tui::pick::first(list.len()))
        .map_or("", String::as_str)
}

#[cfg(test)]
#[path = "opening.rs"]
mod opening_tests;

#[cfg(test)]
#[path = "panes.rs"]
mod panes;
