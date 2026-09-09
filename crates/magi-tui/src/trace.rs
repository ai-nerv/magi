//! What happened in this session, in the order it happened.
//!
//! Rows come from [`HarnessEvent`]s the client was already receiving: the turns, calls, questions,
//! answers, compactions and retries the transcript deliberately hides. Recorded from startup
//! whether or not `:trace` is ever opened, in a ring of [`KEEP`] rows, oldest dropped first.

use magi_proto::HarnessEvent;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::VecDeque;

/// How many rows to keep. Anything further back is the journal's job.
pub const KEEP: usize = 1_000;

/// One thing that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    pub what: String,
    /// Anything worth showing beside it — a duration, a size, an outcome.
    pub detail: String,
}

/// What sort of thing a row is. A closed set: the chip is two to four characters and is learnt by
/// reading the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Turn,
    Tool,
    Permit,
    Context,
    Model,
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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

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

    /// Note whatever this event was, if it is one the trace is about. Not `AssistantDelta`, which
    /// arrives many times a second and would flush the ring within one reply.
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
            _ => return,
        };
        self.push(row);
    }

    /// The rows, drawn in fixed columns so the chip can be read down.
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

/// `text`, cut to `width` on a character boundary rather than a byte one.
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
