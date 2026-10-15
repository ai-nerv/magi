//! The transcript as a stack of blocks. Each block keeps the rows it last drew, and draws again only
//! when its entry, the width or how much of it is shown changes.

use super::{Detail, blank_row, entry_lines};
use magi_proto::{Entry, ToolCallId};
use ratatui::text::Line;
use std::collections::BTreeSet;
use std::ops::Range;

/// One entry, as it was last drawn.
struct Slot {
    entry: Entry,
    shown: Detail,
    width: u16,
    rows: Vec<Line<'static>>,
    /// A blank row above it, parting it from the block before.
    gap: bool,
}

/// Every entry of a transcript, drawn, and the row each one starts on.
#[derive(Default)]
pub struct Stack {
    slots: Vec<Slot>,
    /// The first row of each slot, its gap included.
    starts: Vec<usize>,
    total: usize,
    drawn: usize,
}

impl Stack {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bring the stack up to `entries` at `width`, drawing again only the blocks that changed.
    /// `flipped` names the tool calls shown the other way from `detail`.
    pub fn update(
        &mut self,
        entries: &[Entry],
        width: u16,
        detail: Detail,
        flipped: &BTreeSet<ToolCallId>,
    ) {
        self.slots.truncate(entries.len());
        self.drawn = 0;
        for (nth, entry) in entries.iter().enumerate() {
            let shown = match entry {
                Entry::Tool { id, .. } if flipped.contains(id) => detail.other(),
                _ => detail,
            };
            let kept = self.slots.get(nth).is_some_and(|slot| {
                slot.width == width && slot.shown == shown && slot.entry == *entry
            });
            if kept {
                continue;
            }
            let slot = Slot {
                entry: entry.clone(),
                shown,
                width,
                rows: entry_lines(entry, width, shown),
                gap: false,
            };
            self.drawn += 1;
            if let Some(old) = self.slots.get_mut(nth) {
                *old = slot;
            } else {
                self.slots.push(slot);
            }
        }
        self.settle();
    }

    /// Part each block from the one before with a blank row, unless either side already is one,
    /// and record where each starts.
    fn settle(&mut self) {
        self.starts.clear();
        let mut total = 0;
        // Whether the last row drawn so far is blank; `None` before anything is drawn.
        let mut last: Option<bool> = None;
        for slot in &mut self.slots {
            slot.gap = match (last, slot.rows.first()) {
                (Some(blank), Some(first)) => !blank && !blank_row(Some(first)),
                _ => false,
            };
            self.starts.push(total);
            total += usize::from(slot.gap) + slot.rows.len();
            if let Some(end) = slot.rows.last() {
                last = Some(blank_row(Some(end)));
            }
        }
        self.total = total;
    }

    /// Rows in the whole transcript, gaps included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.total
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// How many blocks the last update drew rather than kept.
    #[must_use]
    pub fn drawn(&self) -> usize {
        self.drawn
    }

    /// The block row `at` of the transcript belongs to, and which of its rows it is. `None` for a
    /// gap between blocks, or past the end.
    #[must_use]
    pub fn at(&self, at: usize) -> Option<(usize, usize)> {
        if at >= self.total {
            return None;
        }
        let nth = self
            .starts
            .partition_point(|start| *start <= at)
            .checked_sub(1)?;
        let slot = self.slots.get(nth)?;
        let within = (at - self.starts.get(nth)?).checked_sub(usize::from(slot.gap))?;
        (within < slot.rows.len()).then_some((nth, within))
    }

    /// Row `at` of the transcript; `None` for a gap.
    #[must_use]
    pub fn line(&self, at: usize) -> Option<&Line<'static>> {
        let (nth, within) = self.at(at)?;
        self.slots.get(nth)?.rows.get(within)
    }

    /// Rows `range` of the transcript, a gap as an empty row.
    #[must_use]
    pub fn rows(&self, range: Range<usize>) -> Vec<Line<'static>> {
        range
            .map(|at| self.line(at).cloned().unwrap_or_default())
            .collect()
    }

    /// The row block `nth` starts on, and the rows it drew.
    #[must_use]
    pub fn block(&self, nth: usize) -> Option<(usize, &[Line<'static>])> {
        let slot = self.slots.get(nth)?;
        Some((
            self.starts.get(nth)? + usize::from(slot.gap),
            slot.rows.as_slice(),
        ))
    }

    /// The tool call block `nth` draws, when it draws one.
    #[must_use]
    pub fn call(&self, nth: usize) -> Option<&ToolCallId> {
        match &self.slots.get(nth)?.entry {
            Entry::Tool { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The block drawing tool call `id`.
    #[must_use]
    pub fn find(&self, id: &ToolCallId) -> Option<usize> {
        (0..self.slots.len()).find(|nth| self.call(*nth) == Some(id))
    }
}

#[cfg(test)]
#[path = "stack/tests.rs"]
mod tests;
