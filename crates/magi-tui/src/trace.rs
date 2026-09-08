//! What happened in this session, in the order it happened.
//!
//! **A timeline of things that were already crossing the wire.** Every row here comes from a
//! [`HarnessEvent`] the client was already receiving and already folding into the transcript.
//! The transcript is a *conversation* — it shows what was said and what a tool answered, and it
//! deliberately hides the rest, because a person reading a reply does not want a permission
//! ledger in the middle of it. This is that rest: the turns, the calls, the questions and their
//! answers, the compactions, the retries.
//!
//! **It is kept whether or not anybody asks for it.** `:trace` opens a view of something the
//! session has been recording since it started; a trace that only began when it was opened would
//! be empty exactly when somebody went looking — which is always just after the thing they wanted
//! to see. That costs one bounded ring buffer, and nothing when the view is never opened.
//!
//! **Bounded, because a session is not.** [`KEEP`] rows, oldest dropped first. A trace that grew
//! without limit would be a memory leak with a nice name on it.

use magi_proto::HarnessEvent;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::VecDeque;

/// How many rows to keep.
///
/// Enough that a long session's interesting part is still in it, and small enough that the cost
/// is a rounding error against a transcript. A person chasing something further back than this
/// wants the journal, which keeps everything.
pub const KEEP: usize = 1_000;

/// One thing that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Which kind, for the chip and for filtering later.
    pub kind: Kind,
    /// What happened, in one line.
    pub what: String,
    /// Anything worth showing beside it — a duration, a size, an outcome.
    pub detail: String,
}

/// What sort of thing a row is.
///
/// A closed set on purpose: the chip is two to four characters and a person learns them by
/// reading the column, which stops being possible the moment anything can appear there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A turn began or ended.
    Turn,
    /// A tool was called, or answered.
    Tool,
    /// A permission was asked, granted or refused.
    Permit,
    /// The context window was compacted.
    Context,
    /// The model changed.
    Model,
    /// Something went wrong.
    Error,
}

impl Kind {
    /// The chip, as it is drawn.
    #[must_use]
    pub fn chip(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Tool => "tool",
            Self::Permit => "perm",
            Self::Context => "ctx",
            Self::Model => "model",
            Self::Error => "err",
        }
    }
}

/// Everything this session has done, newest last.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    rows: VecDeque<Row>,
}

impl Trace {
    /// Nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many rows it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Write one down, dropping the oldest when full.
    pub fn push(&mut self, row: Row) {
        if self.rows.len() == KEEP {
            self.rows.pop_front();
        }
        self.rows.push_back(row);
    }

    /// Note whatever this event was, if it is one the trace is about.
    ///
    /// **Not every event.** `AssistantDelta` arrives many times a second and says nothing a
    /// timeline can use; a trace that recorded it would be a transcript with worse formatting and
    /// would push everything else out of the ring within one reply.
    pub fn note(&mut self, event: &HarnessEvent) {
        let row = match event {
            HarnessEvent::AssistantStarted { .. } => Row {
                kind: Kind::Turn,
                what: "turn began".to_owned(),
                detail: String::new(),
            },
            HarnessEvent::AssistantEnded {
                stop_reason, usage, ..
            } => Row {
                kind: Kind::Turn,
                what: format!("turn ended, {stop_reason:?}").to_lowercase(),
                detail: format!(
                    "{} in, {} out",
                    usage.input + usage.cache_read + usage.cache_write,
                    usage.output
                ),
            },
            HarnessEvent::ToolCallStarted { name, .. } => Row {
                kind: Kind::Tool,
                what: name.clone(),
                detail: "called".to_owned(),
            },
            HarnessEvent::ToolCallEnded { result, .. } => Row {
                kind: Kind::Tool,
                what: "answered".to_owned(),
                detail: if result.is_error {
                    "failed".to_owned()
                } else {
                    format!("{} bytes", result.output.len())
                },
            },
            HarnessEvent::PermissionAsked { tool, action, .. } => Row {
                kind: Kind::Permit,
                what: format!("{} {}", action.verb(), action.subject()),
                detail: format!("asked by {tool}"),
            },
            HarnessEvent::Granted { grant, .. } => Row {
                kind: Kind::Permit,
                what: format!("{} granted", grant.verb),
                detail: String::new(),
            },
            HarnessEvent::Refused { message, .. } => Row {
                kind: Kind::Permit,
                what: "refused".to_owned(),
                detail: message.clone(),
            },
            HarnessEvent::Compacted { replaces, .. } => Row {
                kind: Kind::Context,
                what: "compacted".to_owned(),
                detail: format!("{replaces} entries replaced"),
            },
            HarnessEvent::ModelChanged { model, .. } => Row {
                kind: Kind::Model,
                what: model
                    .as_ref()
                    .map_or_else(|| "none".to_owned(), |m| m.name.clone()),
                detail: String::new(),
            },
            HarnessEvent::Error { class, message, .. } => Row {
                kind: Kind::Error,
                what: format!("{class:?}").to_lowercase(),
                detail: message.clone(),
            },
            // Everything else is either the conversation itself or a redraw. See above.
            _ => return,
        };
        self.push(row);
    }

    /// The rows, drawn.
    ///
    /// Fixed columns, because the chip is a thing you learn by reading down it and a column that
    /// moves with its contents cannot be read that way.
    #[must_use]
    pub fn lines(&self) -> Vec<Line<'static>> {
        self.rows
            .iter()
            .map(|row| {
                Line::from(vec![
                    Span::styled(
                        format!("{:<6}", row.kind.chip()),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                    Span::raw(format!("{:<28}", clipped(&row.what, 28))),
                    Span::styled(
                        clipped(&row.detail, 40),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ])
            })
            .collect()
    }
}

/// `text`, cut to `width` on a character boundary.
///
/// By characters rather than bytes: a path with an accent in it would otherwise be cut mid-scalar
/// and the row would render as a replacement character.
fn clipped(text: &str, width: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= width {
        return flat;
    }
    let mut out: String = flat.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "trace/noting.rs"]
mod noting;
