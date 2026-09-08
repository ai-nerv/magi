//! UI state, and the reduction of harness events onto it.
//!
//! Pure: no terminal, no socket. The driver feeds it events and keys and asks it what to
//! draw, which is what lets the whole state machine be tested without a pty.

use magi_proto::{AgentStatus, Cursor, Entry, HarnessEvent, MessageId, ToolCallId};
use magi_tui::Editor;
use magi_tui::overlay::Overlay;
use magi_tui::scrollback::Scrollback;

/// Add two token counts.
///
/// Written out because `Usage` is four independent counters, and summing three while
/// forgetting the fourth shows up as a footer that quietly reads low.
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
    /// Transcript in order.
    entries: Vec<Entry>,
    /// Highest cursor seen, so a reconnect resumes rather than replays.
    cursor: Cursor,
    /// What the agent is doing.
    status: AgentStatus,
    /// The prompt buffer.
    pub editor: Editor,
    /// The transcript, which magi owns: the alternate screen has no terminal history to defer to.
    pub scrollback: Scrollback,

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
    /// What to say when the daemon reports no model, if the UI has worked out something better.
    ///
    /// The fixed sentence is a last resort: it claims nothing is configured, which is false in
    /// the ordinary case of a configured model whose provider key is not set.
    pub no_model: Option<String>,
    /// What the open permission prompt is about.
    ///
    /// Kept because a scope's label is written *in terms of the action* — "any `git` command",
    /// "anything under /home/you/work" — so turning a chosen label back into a scope needs the
    /// action that produced it.
    pub asking_about: magi_proto::permit::Action,
    /// Commands submitted but not yet handed to a daemon.
    ///
    /// Set by the driver from the command channel: a prompt sent while the daemon is away
    /// waits in it rather than being lost, and an emptied prompt box with nothing on screen
    /// gave no way to tell those two apart.
    pub queued: usize,
    /// Spinner phase.
    pub tick: usize,
    /// Scan phase, in hundredths of a tick.
    ///
    /// Its own clock rather than the spinner's, because the scan has a speed somebody can set
    /// and the spinner does not. Hundredths so `scan_speed = 0.5` is half as fast rather than
    /// stopped, which is what it would round to in whole ticks.
    scan_phase: usize,
    /// Which model is answering, as the daemon reported it.
    ///
    /// From the daemon rather than read from the configuration here: a UI reading the config
    /// for itself would name whatever is configured *now*, which after an edit is not what the
    /// daemon on the other end of the socket is actually talking to.
    pub model: Option<magi_proto::ModelInfo>,
    /// How much reasoning is being asked for.
    pub thinking: String,
    /// Whether the model answering can reason at all.
    model_reasons: bool,
    /// Everything the daemon says this session could switch to.
    pub choices: Vec<magi_proto::ModelChoice>,
    /// What is open under the prompt: a list, a completion popup, or nothing.
    ///
    /// One slot rather than two. They were never open together — a list is opened by a command,
    /// and running a command closes the popup that offered it — and holding that apart in a
    /// comment while every reader checked both fields is how the two drifted into two heights,
    /// two draw calls and two looks.
    pub overlay: Option<Overlay>,
    /// A panel in the middle, over the transcript.
    ///
    /// A different thing from [`App::overlay`], which is anchored to the prompt because it is
    /// about the line being typed. A float is a view of what the session knows, opened by name.
    pub pane: Option<magi_tui::pane::Pane>,
    /// Everything this session has done, kept whether or not anybody looks.
    ///
    /// **Recorded from the start rather than from when `:trace` is opened.** A trace that began
    /// when it was asked for would be empty exactly when somebody went looking, which is always
    /// just after the thing they wanted to see. It costs one bounded ring — see
    /// [`magi_tui::trace::KEEP`].
    ///
    /// Named for what it is rather than `trace`, which this struct already spends on the status
    /// line's scroller.
    pub timeline: magi_tui::trace::Trace,
    /// What this project is made of, once something has indexed it.
    ///
    /// Empty until then, and [`magi_tui::graph::Graph::empty`] says how to fill it.
    pub graph: magi_tui::graph::Graph,
    /// What that list is choosing.
    ///
    /// Held beside the list rather than inside it, because the list is a generic widget and
    /// this is the one thing about it only its opener knows. Without it every list's answer
    /// went to the same place, and picking a thinking level asked for a model called "medium".
    pub picking: Option<Picking>,
    /// Rows a tool is holding, and the last frame it drew in them.
    ///
    /// One at a time: a turn runs its tool calls in order, so two tools cannot be holding the
    /// screen at once, and a list would be a queue nothing ever puts a second thing in.
    pub surface: Option<surfacing::Surfacing>,
    /// How much of each tool result to show.
    pub detail: magi_tui::transcript::Detail,
    /// What this session is called, as the footer shows it.
    ///
    /// `project/role/id` when melchior is running, because naming is its job: it holds the directory
    /// those names live in and can look before it chooses. Just the project otherwise — with no
    /// layer there are no siblings to be told apart.
    pub named: String,
    /// The empty prompt writing to itself.
    ///
    /// Held here rather than in the renderer because it moves on a clock and on what the person
    /// is doing, neither of which a draw call knows about. See [`App::settle_prompt`].
    pub tease: magi_tui::tease::Tease,
    /// The scramble a newly opened list lands with.
    ///
    /// A field rather than a static, unlike the opening one: the screen opens once and a list
    /// opens every time you ask for a model or answer a permission.
    pub landing: magi_tui::decrypt::Landing,
    /// The footer's trace, and everything that has scrolled past on it.
    pub trace: magi_tui::beacon::Trace,
    /// Which mode the prompt is in, and any half-typed command waiting on its second key.
    pub modal: crate::keys::Modal,
    /// Every session melchior says is listening in this project, for the `$` popup.
    ///
    /// Pushed by melchior rather than read here: a completion offered on a keystroke cannot go and
    /// look, and magi reading the directory would be a second place that knows the layout.
    pub reachable: Vec<String>,
    /// How many messages from other sessions have arrived and not been answered.
    ///
    /// A count, not the messages. What was said goes into the transcript like anything else,
    /// and what is *unanswered* is the only part a sibling asking `status` cares about —
    /// everything else about an inbox belongs to the layer that holds it.
    pub waiting: usize,
    /// What this session is allowed to do, as far as the screen has seen it decided.
    ///
    /// Kept here rather than asked of the session, because the UI is where every one of them was
    /// decided: the configured rules are read at startup, and each later grant is a picker answer
    /// this loop sent. The ledger the session actually enforces with lives on the worker thread,
    /// behind a lock, and going to fetch it would be a round trip for something already known.
    ///
    /// It exists for one purpose: handing it to a session this one takes on as a child. A child
    /// gets what its parent already holds and nothing more, so this is that list.
    pub granted: Vec<magi_proto::permit::Grant>,
    /// Whether the prompt was empty when it was last looked at.
    was_blank: bool,
    /// Which transcript line and column the pointer is over, when it is over a handle.
    ///
    /// A transcript coordinate rather than a screen one, so scrolling carries the highlight with
    /// the block it belongs to instead of leaving it on whatever moved under it.
    pub hovering: Option<(usize, u16)>,
    /// The text being dragged over, or the last drag that finished.
    ///
    /// Kept after the button comes up so the highlight stays until the next click, which is how
    /// a person checks they got what they meant before pasting it.
    pub selection: Option<magi_tui::select::Selection>,
    /// Tool blocks showing the opposite of `detail`, because they were clicked.
    ///
    /// Membership rather than an absolute state, so the fold key still moves every block a
    /// person has not had an opinion about, and every block they have keeps the one they gave.
    pub flipped: std::collections::BTreeSet<ToolCallId>,
    /// Which tool call each rendered line belongs to, parallel to the scrollback.
    pub owners: Vec<Option<ToolCallId>>,
    /// Which entry drew each rendered line, parallel to the scrollback.
    ///
    /// Every block, not only the ones that fold: a copy chip has to gather the rows of the block
    /// it sits in, and an assistant message has no id to key that on.
    pub blocks: Vec<Option<usize>>,
    /// Which screen rows the transcript occupies, so a click can be turned into a line.
    ///
    /// Recorded by the drawing pass because only it knows: the live region ends where the
    /// prompt begins, and the prompt grows with what has been typed into it.
    pub live_rows: std::ops::Range<u16>,
    /// Where the rows a tool is holding landed on screen, when one is holding any.
    ///
    /// The whole of what magi knows about a surface's position, and the whole of what a tenant is
    /// never told: this is how a click at row 31 becomes "row 2 of your own rows", and the tenant
    /// gets the second half of that sentence. `None` when nothing is holding rows, so a pointer
    /// over an ordinary picker is not translated into coordinates for a surface that has closed.
    pub surface_rect: Option<ratatui::layout::Rect>,
    /// Where the corner badge is on screen, so a press on it opens what it is about.
    ///
    /// The thing you are looking at when you wonder about it is the one worn by the prompt box;
    /// pressing it is the shortest path from noticing to knowing. `None` when the corner is
    /// wearing nothing, which is a session that has nothing to say there yet.
    pub corner_rect: Option<ratatui::layout::Rect>,
    /// What the corner is showing, and therefore what pressing it opens.
    ///
    /// One slot, one setting. See [`magi_tui::corner::Corner`].
    pub corner: magi_tui::corner::Corner,
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
            pane: None,
            timeline: magi_tui::trace::Trace::new(),
            graph: magi_tui::graph::Graph::new(),
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
            // Folded. A transcript of whole build logs is not a transcript, and the handle at
            // the foot of each block is how you open the one you care about.
            detail: magi_tui::transcript::Detail::Preview,
            named: String::new(),
            tease: magi_tui::tease::Tease::new(opener()),
            landing: magi_tui::decrypt::Landing::default(),
            trace: magi_tui::beacon::Trace::default(),
            modal: crate::keys::Modal::default(),
            reachable: Vec::new(),
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

    /// Move the empty prompt on, and put it back to an opener when somebody types.
    ///
    /// A prompt that has just emptied -- deleted back to nothing, or submitted -- starts its
    /// wait over with a fresh opener. A prompt with something in it shows no placeholder at all,
    /// so there is nothing to advance.
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
    pub fn usage(&self) -> magi_proto::Usage {
        self.entries
            .iter()
            .fold(magi_proto::Usage::default(), |total, entry| match entry {
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
    /// message from magi about itself rather than the beginning of a conversation -- and
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

    /// Drop the transcript without touching the daemon.
    ///
    /// `/clear` hides history from the view; it does not delete it. The journal is
    /// append-only, and a UI command must never be able to rewrite it.
    pub fn clear_view(&mut self) {
        self.entries.clear();
    }

    /// Append a local notice to the transcript.
    ///
    /// Notices are UI-side only and never reach the journal: `:help` output is not something
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
        let choices: Vec<magi_tui::picker::Choice> = self
            .choices
            .iter()
            // "set OPENROUTER_API_KEY" used to be a lie told to somebody who had set it an hour
            // ago: the daemon captured its environment at start and outlived the shell that
            // started it, so a key exported afterwards never reached it, and this was the only
            // process that could tell the two apart. There is no daemon now — the session is
            // this process — so what it can see and what the catalog was built from are the
            // same environment, always.
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

    /// Show every line of each tool result, or go back to the preview.
    ///
    /// A whole transcript at a time rather than one block: picking a block needs a selection,
    /// and a selection needs keys, a highlight and a rule for what happens when the thing
    /// selected scrolls away. The question being asked is almost always about the last result.
    pub fn toggle_detail(&mut self) -> magi_tui::transcript::Detail {
        self.detail = match self.detail {
            magi_tui::transcript::Detail::Preview => magi_tui::transcript::Detail::Full,
            magi_tui::transcript::Detail::Full => magi_tui::transcript::Detail::Preview,
        };
        self.detail
    }

    /// Recompute the completion popup from the current prompt.
    ///
    /// The command menu only opens on the command line. A colon typed in insert mode is a
    /// colon -- in a sentence, in a path, in a ratio -- and it used to put the command palette
    /// over the prompt every time somebody wrote one.
    pub fn refresh_completion(&mut self, list_paths: &dyn Fn(&str) -> Vec<String>) {
        let (row, col) = self.editor.cursor();
        let line = self.editor.lines()[row].clone();
        // `$` offers whoever is listening. Read from the socket directory on the keystroke
        // rather than from a list kept up to date, because an instance that died did not get to
        // remove itself from one.
        let resolved =
            magi_tui::complete::resolve_with(&line, col, list_paths, &|_| self.reachable.clone());
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

mod kin;
mod picking;
pub use picking::Picking;
/// Applying one harness event to the state.
mod applying;
mod asked;
mod folding;
#[cfg(test)]
mod retracting;
mod sessions;
pub mod surfacing;
#[cfg(test)]
mod tests;
/// Opening the info pane: which view, and what goes in it.
#[path = "views.rs"]
mod views;

/// A line for the box to open with.
///
/// Drawn fresh each time the prompt empties, so sitting down twice does not read the same twice.
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

/// Opening the info pane, and what each view puts in it.
#[cfg(test)]
#[path = "panes.rs"]
mod panes;
