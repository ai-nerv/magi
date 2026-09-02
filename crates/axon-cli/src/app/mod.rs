//! UI state, and the reduction of harness events onto it.
//!
//! Pure: no terminal, no socket. The driver feeds it events and keys and asks it what to
//! draw, which is what lets the whole state machine be tested without a pty.

use axon_proto::{AgentStatus, Cursor, Entry, HarnessEvent, MessageId, ToolCallId};
use axon_tui::Editor;
use axon_tui::overlay::Overlay;
use axon_tui::scrollback::Scrollback;

/// Add two token counts.
///
/// Written out because `Usage` is four independent counters, and summing three while
/// forgetting the fourth shows up as a footer that quietly reads low.
fn add(total: axon_proto::Usage, next: axon_proto::Usage) -> axon_proto::Usage {
    axon_proto::Usage {
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
    /// The transcript, which axon owns: the alternate screen has no terminal history to defer to.
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
    pub asking_about: axon_proto::permit::Action,
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
    pub model: Option<axon_proto::ModelInfo>,
    /// How much reasoning is being asked for.
    pub thinking: String,
    /// Whether the model answering can reason at all.
    model_reasons: bool,
    /// Everything the daemon says this session could switch to.
    pub choices: Vec<axon_proto::ModelChoice>,
    /// What is open under the prompt: a list, a completion popup, or nothing.
    ///
    /// One slot rather than two. They were never open together — a list is opened by a command,
    /// and running a command closes the popup that offered it — and holding that apart in a
    /// comment while every reader checked both fields is how the two drifted into two heights,
    /// two draw calls and two looks.
    pub overlay: Option<Overlay>,
    /// What that list is choosing.
    ///
    /// Held beside the list rather than inside it, because the list is a generic widget and
    /// this is the one thing about it only its opener knows. Without it every list's answer
    /// went to the same place, and picking a thinking level asked for a model called "medium".
    pub picking: Option<Picking>,
    /// How much of each tool result to show.
    pub detail: axon_tui::transcript::Detail,
    /// What this session is called, as the footer shows it.
    ///
    /// `project/role/id` when atom is running, because naming is its job: it holds the directory
    /// those names live in and can look before it chooses. Just the project otherwise — with no
    /// layer there are no siblings to be told apart.
    pub named: String,
    /// The empty prompt writing to itself.
    ///
    /// Held here rather than in the renderer because it moves on a clock and on what the person
    /// is doing, neither of which a draw call knows about. See [`App::settle_prompt`].
    pub tease: axon_tui::tease::Tease,
    /// The scramble a newly opened list lands with.
    ///
    /// A field rather than a static, unlike the opening one: the screen opens once and a list
    /// opens every time you ask for a model or answer a permission.
    pub landing: axon_tui::decrypt::Landing,
    /// The footer's trace, and everything that has scrolled past on it.
    pub trace: axon_tui::beacon::Trace,
    /// Which mode the prompt is in, and any half-typed command waiting on its second key.
    pub modal: crate::keys::Modal,
    /// Every session atom says is listening in this project, for the `$` popup.
    ///
    /// Pushed by atom rather than read here: a completion offered on a keystroke cannot go and
    /// look, and axon reading the directory would be a second place that knows the layout.
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
    pub granted: Vec<axon_proto::permit::Grant>,
    /// Whether the prompt was empty when it was last looked at.
    was_blank: bool,
    /// The text being dragged over, or the last drag that finished.
    ///
    /// Kept after the button comes up so the highlight stays until the next click, which is how
    /// a person checks they got what they meant before pasting it.
    pub selection: Option<axon_tui::select::Selection>,
    /// Tool blocks showing the opposite of `detail`, because they were clicked.
    ///
    /// Membership rather than an absolute state, so the fold key still moves every block a
    /// person has not had an opinion about, and every block they have keeps the one they gave.
    pub flipped: std::collections::BTreeSet<ToolCallId>,
    /// Which tool call each rendered line belongs to, parallel to the scrollback.
    pub owners: Vec<Option<ToolCallId>>,
    /// Which screen rows the transcript occupies, so a click can be turned into a line.
    ///
    /// Recorded by the drawing pass because only it knows: the live region ends where the
    /// prompt begins, and the prompt grows with what has been typed into it.
    pub live_rows: std::ops::Range<u16>,
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
            detail: axon_tui::transcript::Detail::Preview,
            named: String::new(),
            tease: axon_tui::tease::Tease::new(opener()),
            landing: axon_tui::decrypt::Landing::default(),
            trace: axon_tui::beacon::Trace::default(),
            modal: crate::keys::Modal::default(),
            reachable: Vec::new(),
            waiting: 0,
            granted: Vec::new(),
            was_blank: true,
            selection: None,
            flipped: std::collections::BTreeSet::new(),
            owners: Vec::new(),
            live_rows: 0..0,
            pending_notice: None,
            no_model: None,
            asking_about: axon_proto::permit::Action::Read {
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
            self.tease.advance(axon_tui::glyph::placeholders());
        }
        self.was_blank = blank;
    }

    /// Advance both clocks by one frame.
    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.scan_phase = self
            .scan_phase
            .wrapping_add(usize::from(axon_tui::metric::scan_speed()));
    }

    /// The scan's phase in whole ticks, which is what the border is drawn from.
    #[must_use]
    pub fn scan_tick(&self) -> usize {
        self.scan_phase / usize::from(axon_tui::metric::NORMAL)
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
    pub fn usage(&self) -> axon_proto::Usage {
        self.entries
            .iter()
            .fold(axon_proto::Usage::default(), |total, entry| match entry {
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
    /// message from axon about itself rather than the beginning of a conversation -- and
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
                cursor: _,
                entries,
                status,
                model,
                choices,
                thinking,
                ..
            } => {
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
                    let said = self.no_model.clone().unwrap_or_else(|| {
                        "No model is configured. Type `:model` to choose one.".to_owned()
                    });
                    self.show_notice(said);
                }
            }
            HarnessEvent::UserMessage { id, text, .. } => {
                // No aside. It is context for the model and the transcript never shows it, so
                // there is nothing here for one to be.
                self.entries.push(Entry::User {
                    id,
                    text,
                    aside: String::new(),
                });
            }
            // Drawn from the session's own stream rather than appended when it landed on the
            // socket. The UI is where a message arrives and the session is where it *is*: an
            // entry the UI kept for itself was one the model never saw, so an instance could be
            // asked a question and sit there until somebody typed at it.
            HarnessEvent::MessageArrived {
                who,
                kin,
                sort,
                text,
                ..
            } => {
                self.entries.push(Entry::From {
                    who,
                    kin,
                    sort,
                    text,
                });
            }
            // Beginning a message that is already on screen means beginning it *again*: an
            // attempt streamed half an answer, failed, and the retry starts from nothing. So it
            // empties the one that is there rather than pushing a second — which is what a
            // retry mid-answer used to leave behind, two copies of the same half-message.
            HarnessEvent::AssistantStarted { id, .. } => {
                if let Some(Entry::Assistant {
                    text,
                    thinking,
                    stop_reason,
                    error,
                    ..
                }) = self.assistant_mut(&id)
                {
                    text.clear();
                    thinking.clear();
                    *stop_reason = None;
                    *error = None;
                } else {
                    self.entries.push(Entry::Assistant {
                        id,
                        text: String::new(),
                        thinking: String::new(),
                        stop_reason: None,
                        error: None,
                        signatures: axon_proto::Signatures::default(),
                        usage: axon_proto::Usage::default(),
                    });
                }
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
            // The turn is blocked until this is answered, so it takes the screen: a picker
            // opened over whatever else was there, with the narrowest answer under the cursor.
            HarnessEvent::PermissionAsked {
                id,
                tool,
                action,
                offers,
                ..
            } => {
                let choices = offers
                    .iter()
                    .map(|scope| axon_tui::picker::Choice {
                        value: scope.label(&action),
                        detail: String::new(),
                        ready: true,
                    })
                    .chain(std::iter::once(axon_tui::picker::Choice {
                        value: "no".to_owned(),
                        detail: "refuse, and tell the model".to_owned(),
                        ready: true,
                    }))
                    .collect();
                self.overlay = Some(
                    axon_tui::picker::Picker::new(
                        format!("{tool} wants to {} {}", action.verb(), action.subject()),
                        choices,
                        None,
                    )
                    .into(),
                );
                self.asking_about = action;
                self.picking = Some(Picking::Permission { id, offers });
            }
            HarnessEvent::ModelChanged { model, .. } => {
                let before = self.model.as_ref().map(|m| m.name.clone());
                let after = model.as_ref().map(|m| m.name.clone());
                if self.started()
                    && before != after
                    && let Some(name) = after
                {
                    self.show_notice(format!("Model is now `{name}`."));
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
                    stop_reason: Some(axon_proto::StopReason::Error),
                    error: Some(format!("{class:?}: {message}")),
                    signatures: axon_proto::Signatures::default(),
                    usage: axon_proto::Usage::default(),
                });
            }
        }
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
        let choices: Vec<axon_tui::picker::Choice> = self
            .choices
            .iter()
            // "set OPENROUTER_API_KEY" used to be a lie told to somebody who had set it an hour
            // ago: the daemon captured its environment at start and outlived the shell that
            // started it, so a key exported afterwards never reached it, and this was the only
            // process that could tell the two apart. There is no daemon now — the session is
            // this process — so what it can see and what the catalog was built from are the
            // same environment, always.
            .map(|choice| axon_tui::picker::Choice {
                value: choice.name.clone(),
                detail: if choice.requirement.is_empty() {
                    axon_tui::footer::format_tokens(choice.context_window)
                } else {
                    choice.requirement.clone()
                },
                ready: choice.requirement.is_empty(),
            })
            .collect();
        let current = self.model.as_ref().map(|m| m.name.clone());
        let picker = axon_tui::picker::Picker::new("Model", choices, current.as_deref());
        if picker.offers_nothing() {
            self.show_notice(
                "No providers are declared. `axon models --all` lists what axon ships.".to_owned(),
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
            .map(|(value, detail)| axon_tui::picker::Choice {
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
            axon_tui::picker::Picker::new("Thinking", choices, Some(self.thinking.as_str())).into(),
        );
        self.picking = Some(Picking::Thinking);
    }

    /// Show every line of each tool result, or go back to the preview.
    ///
    /// A whole transcript at a time rather than one block: picking a block needs a selection,
    /// and a selection needs keys, a highlight and a rule for what happens when the thing
    /// selected scrolls away. The question being asked is almost always about the last result.
    pub fn toggle_detail(&mut self) -> axon_tui::transcript::Detail {
        self.detail = match self.detail {
            axon_tui::transcript::Detail::Preview => axon_tui::transcript::Detail::Full,
            axon_tui::transcript::Detail::Full => axon_tui::transcript::Detail::Preview,
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
            axon_tui::complete::resolve_with(&line, col, list_paths, &|_| self.reachable.clone());
        self.overlay = resolved
            .filter(|found| {
                found.kind != axon_tui::complete::Kind::Command || self.modal.commanding()
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
#[cfg(test)]
mod retracting;
#[cfg(test)]
mod tests;

impl App {
    /// Offer the sessions recorded in this directory.
    ///
    /// Read here rather than asked of the daemon: the journals are files on this machine, this
    /// process is on the same machine, and a round trip to be told what a directory listing says
    /// would be a protocol message that earns nothing.
    pub fn open_session_picker(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let dir = axon_host::paths::sessions_dir();
        let found = axon_host::paths::summaries(&dir, &cwd.display().to_string());
        if found.is_empty() {
            self.show_notice(
                "No earlier sessions in this directory. This one is the first.".to_owned(),
            );
            return;
        }

        let choices: Vec<axon_tui::picker::Choice> = found
            .iter()
            .map(|found| axon_tui::picker::Choice {
                // What it was for, which is the only thing anybody recognises a session by.
                // Nobody titles one, so the opening prompt stands in for a title.
                value: if found.title.is_empty() {
                    "(nothing was asked)".to_owned()
                } else {
                    found.title.clone()
                },
                detail: format!("{} entries", found.entries),
                ready: true,
            })
            .collect();
        self.overlay = Some(
            axon_tui::picker::Picker::new("Continue which session?", choices.clone(), None).into(),
        );
        self.picking = Some(Picking::Session {
            rows: choices
                .iter()
                .map(|choice| choice.value.clone())
                .zip(found.into_iter().map(|found| found.id))
                .collect(),
        });
    }
}

mod folding;

/// A line for the box to open with.
///
/// Drawn fresh each time the prompt empties, so sitting down twice does not read the same twice.
pub(crate) fn opener() -> &'static str {
    let list = axon_tui::glyph::openers();
    if list.is_empty() {
        return "";
    }
    list.get(axon_tui::pick::first(list.len()))
        .map_or("", String::as_str)
}

#[cfg(test)]
#[path = "opening.rs"]
mod opening_tests;
