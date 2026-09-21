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
        cost_micros: total.cost_micros + next.cost_micros,
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
    /// Which run this is, as the session last said. `:rename` needs it, and nothing else does.
    pub session_id: Option<String>,
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
    /// Who is asked about what no rule covers, and on what terms, as the session last said.
    pub judging: magi_proto::judging::Judging,
    /// Which provider serves the model, by routing tag, as chosen on its card; `None` is the router's.
    pub provider: Option<String>,
    /// Which model answered each finished turn, as it was when the turn ended.
    pub turn_models: std::collections::HashMap<MessageId, String>,
    model_reasons: bool,
    pub choices: Vec<magi_proto::ModelChoice>,
    /// A list or a completion popup. One slot: running a command closes the popup that offered it.
    pub overlay: Option<Overlay>,
    pub pane: Option<magi_tui::pane::Pane>,
    /// Recorded from the start whether or not anybody looks, in one bounded ring.
    pub timeline: magi_tui::trace::Trace,
    pub picking: Option<Picking>,
    /// Every question the session has open, oldest first; `picking` is the one on screen.
    pub asks: Vec<HarnessEvent>,
    /// One at a time: a turn runs its tool calls in order.
    pub surface: Option<surfacing::Surfacing>,
    pub detail: magi_tui::transcript::Detail,
    /// `project/role/id` when melchior is running, which does the naming; the project alone otherwise.
    pub named: String,
    pub tease: magi_tui::tease::Tease,
    pub landing: magi_tui::decrypt::Landing,
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
    /// Where the footer name landed: a press on it opens the agents view. `None` before the first
    /// draw records it.
    pub name_rect: Option<ratatui::layout::Rect>,
    /// Whether the pointer is over that name, so the footer can draw it inverted like the usage badge.
    pub name_hover: bool,
    /// Where the model's name landed on the footer, which opens its card; and whether the pointer is on it.
    pub model_rect: Option<ratatui::layout::Rect>,
    pub model_hover: bool,
    /// Whether melchior, balthasar and casper are up, in that order, for the footer's three dots.
    pub siblings: [bool; 3],
    /// When each of those three last did something, for the flash on its dot.
    pub stirred: [Option<std::time::Instant>; 3],
    /// Where each of those three landed on the footer, for the pointer.
    pub sibling_rects: [Option<ratatui::layout::Rect>; 3],
    /// Which sibling's dot the pointer is on: drawn inverted, the way the name shows it is a button.
    pub sibling_hover: Option<usize>,
    /// Agents whose branch is shut in the agents view, by id. Kept here rather than in the view, so a
    /// fold survives the view being rebuilt or reopened.
    pub folded: std::collections::BTreeSet<String>,
    /// What each configured role is for, by name, for the agents view to say.
    pub about: std::collections::BTreeMap<String, String>,
    /// Which program owns the model, asked for a model's card.
    pub mind: String,
    /// Which program offers the tools, asked for casper's float.
    pub tools_program: String,
    /// What the provider published about a model, by its name, once asked; and an answer on its way.
    pub details: Option<(String, Result<magi_tui::model_card::Details, String>)>,
    pub details_rx: Option<std::sync::mpsc::Receiver<crate::app::views::Answered>>,
    /// An id `--attach` named to watch: held until that agent appears on the roster, then the screen
    /// points at it and this clears. `None` for an ordinary session.
    pub attach_wanted: Option<String>,
    /// Started with `--view-only`: nothing this screen sends may change a session.
    pub view_only: bool,
    pub corner: magi_tui::corner::Corner,
    /// How the last request was laid out, as the session last said.
    pub laid: Option<magi_tui::laid::Laid>,
    /// Every helper job this screen has seen finish, for the cost view.
    pub helped: Vec<magi_tui::cost::Helper>,
    /// The project's notes and their change log, as the memory layer last answered.
    pub notes: Option<serde_json::Value>,
    pub changes: Vec<serde_json::Value>,
    /// What the tools program says it offers, as casper's float draws it. `None` until it has been
    /// asked, which happens once when that float is first opened.
    pub tools: Option<Vec<magi_tui::tooling::Tool>>,
    pub tools_rx: Option<std::sync::mpsc::Receiver<Vec<magi_tui::tooling::Tool>>>,
    /// The memory float's tabs, each as the layer last answered: what it holds, the runs it has
    /// seen, and — for whichever memory the cursor is on — what that one has been worth.
    pub memories: Option<Vec<serde_json::Value>>,
    pub runs: Option<Vec<serde_json::Value>>,
    pub utility: Option<serde_json::Value>,
    pub why_of: Option<serde_json::Value>,
    /// Which memory `utility` and `why_of` are about, so a stale pair is not drawn under a new one.
    pub worth_of: Option<String>,
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
            judging: magi_proto::judging::Judging::default(),
            provider: None,
            turn_models: std::collections::HashMap::new(),
            model_reasons: false,
            choices: Vec::new(),
            overlay: None,
            picking: None,
            asks: Vec::new(),
            // Folded; the handle at the foot of each block opens the one you care about.
            detail: magi_tui::transcript::Detail::Preview,
            named: String::new(),
            tease: magi_tui::tease::Tease::new(opener()),
            landing: magi_tui::decrypt::Landing::default(),
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
            name_rect: None,
            name_hover: false,
            model_rect: None,
            model_hover: false,
            siblings: [false; 3],
            stirred: [None; 3],
            sibling_rects: [None; 3],
            sibling_hover: None,
            folded: std::collections::BTreeSet::new(),
            about: std::collections::BTreeMap::new(),
            mind: "melchior".to_owned(),
            tools_program: "casper".to_owned(),
            details: None,
            details_rx: None,
            attach_wanted: None,
            view_only: false,
            corner: magi_tui::corner::Corner::default(),
            laid: None,
            helped: Vec::new(),
            notes: None,
            tools: None,
            tools_rx: None,
            memories: None,
            runs: None,
            utility: None,
            why_of: None,
            worth_of: None,
            changes: Vec::new(),
            pending_notice: None,
            no_model: None,
            session_id: None,
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

    /// Flash a sibling's dot: 0 melchior, 1 balthasar, 2 casper.
    pub fn stir(&mut self, nth: usize) {
        if let Some(at) = self.stirred.get_mut(nth) {
            *at = Some(std::time::Instant::now());
        }
    }

    /// Flash whichever siblings an event says just worked: the model turns through melchior, the
    /// scribe files every entry with balthasar, and every tool but memory's is casper's.
    pub(super) fn stir_for(&mut self, event: &HarnessEvent) {
        let memory = |name: &str| matches!(name, "recall" | "remember" | "forget" | "why");
        let (mel, bal, cas) = match event {
            HarnessEvent::AssistantStarted { .. } | HarnessEvent::AssistantDelta { .. } => {
                (true, false, false)
            }
            HarnessEvent::MessageArrived { .. } => (true, true, false),
            HarnessEvent::UserMessage { .. }
            | HarnessEvent::AssistantEnded { .. }
            | HarnessEvent::ContextLaid { .. }
            | HarnessEvent::Compacted { .. } => (false, true, false),
            HarnessEvent::HelperSpent { .. } => (true, true, false),
            HarnessEvent::ToolCallStarted { name, .. } => (false, memory(name), !memory(name)),
            HarnessEvent::ToolCallEnded { id, .. } => {
                let remembered = self
                    .tool_mut(id)
                    .is_some_and(|entry| matches!(entry, Entry::Tool { name, .. } if memory(name)));
                (false, true, !remembered)
            }
            HarnessEvent::Surfaced { .. } | HarnessEvent::PermissionAsked { .. } => {
                (false, false, true)
            }
            _ => (false, false, false),
        };
        for (nth, stirred) in [mel, bal, cas].into_iter().enumerate() {
            if stirred {
                self.stir(nth);
            }
        }
    }

    /// Each dot's light this frame, like a drive's activity lamp: lit the moment its sibling works,
    /// then blinking at random while it stays busy, more often the more recently it worked, and dark
    /// once it has stopped.
    #[must_use]
    pub fn stirring(&self) -> [f32; 3] {
        const BUSY_SECS: f32 = 1.2;
        let mut lit = [0.0; 3];
        for (nth, at) in self.stirred.iter().enumerate() {
            let Some(at) = at else { continue };
            let since = at.elapsed().as_secs_f32();
            if since >= BUSY_SECS {
                continue;
            }
            let busy = 1.0 - since / BUSY_SECS;
            if since < 0.08 || Self::roll(self.scan_phase, nth) < 0.3 + 0.5 * busy {
                lit[nth] = 1.0;
            }
        }
        lit
    }

    /// A fresh number in `0..1` for each frame and dot: the flicker's dice, the same on a redraw.
    fn roll(frame: usize, nth: usize) -> f32 {
        let seed = u64::try_from(frame)
            .unwrap_or(0)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ u64::try_from(nth + 1)
                .unwrap_or(1)
                .wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let mixed = (seed ^ (seed >> 31)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let mixed = mixed ^ (mixed >> 29);
        f32::from(u16::try_from(mixed % 1000).unwrap_or(0)) / 1000.0
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

    /// The coarse phase this session reports to the roster — what a coordinator reads off the tree.
    /// Working while a turn runs; blocked (with a one-line cause) when the last turn errored; idle
    /// otherwise. Mechanical: derived from turn state, never from anything the model chose to say.
    /// `finished` belongs to the headless path, which knows its assigned work is done — not here,
    /// where an interactive session going quiet only means "ready for more".
    #[must_use]
    pub fn phase(&self) -> (magi_proto::Phase, Option<String>) {
        use magi_proto::Phase;
        if self.is_busy() {
            return (Phase::Working, None);
        }
        if let Some(Entry::Assistant {
            error: Some(why), ..
        }) = self.entries.last()
        {
            let cause = why.lines().next().unwrap_or(why).chars().take(80).collect();
            return (Phase::Blocked, Some(cause));
        }
        (Phase::Idle, None)
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
pub use asks::marked;
pub use crewing::{Seat, changes, for_screen};
pub(crate) use kin::relation;
mod picking;
pub use picking::Picking;
mod applying;
mod asked;
mod asks;
mod folding;
mod resetting;
#[cfg(test)]
mod retracting;
mod sessions;
mod siblings;
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
