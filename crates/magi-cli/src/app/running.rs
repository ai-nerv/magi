//! Blocks a tool draws in and leaves running: where each is drawn, pinning them above the prompt,
//! finding them again from the list of running blocks, and the flash that says which one it is.
//! A running block is drawn in one place at a time: its call's block, the dock, or the float.

use super::App;
use super::surfacing::Surfacing;
use magi_proto::ToolCallId;
use magi_proto::tooling::Place;
use magi_tui::transcript::Live;

/// How long a block picked while it is on screen flashes: twice, on and off.
const FLASH: std::time::Duration = std::time::Duration::from_millis(600);
const BEAT: u128 = 150;

/// What each surface drawn in its call's block is drawn from, and what its block says instead while
/// it is drawn somewhere else.
pub(super) fn lives<'a>(surfaces: &'a [Surfacing], focus: Option<&ToolCallId>) -> Vec<Live<'a>> {
    surfaces
        .iter()
        .filter(|held| held.place == Place::Block)
        .filter_map(|held| {
            let away = if held.pinned {
                Some("pinned above the prompt ↓")
            } else if held.viewing {
                Some("open in the float")
            } else {
                None
            };
            Some(Live {
                call: held.call.as_ref()?,
                tool: &held.tool,
                title: &held.about,
                drawn: &held.drawn,
                away,
                pin: held.detach.then_some(held.pinned),
                focused: focus == Some(&held.id),
            })
        })
        .collect()
}

impl App {
    /// The surfaces that answered their calls and run on.
    pub fn running(&self) -> impl Iterator<Item = &Surfacing> {
        self.surfaces.iter().filter(|held| held.detach)
    }

    /// What a pinned block above the prompt is drawn from, and whether it is open.
    #[must_use]
    pub fn docking(&self) -> Vec<(Live<'_>, bool, ToolCallId)> {
        lives(&self.surfaces, self.focus.as_ref())
            .into_iter()
            .zip(
                self.surfaces
                    .iter()
                    .filter(|held| held.place == Place::Block),
            )
            .filter(|(_, held)| held.pinned)
            .map(|(live, held)| (Live { away: None, ..live }, held.open, held.id.clone()))
            .collect()
    }

    /// Pin a running block above the prompt, or put it back in the conversation.
    pub fn toggle_pin(&mut self, id: &ToolCallId) {
        if let Some(held) = self
            .surfaces
            .iter_mut()
            .find(|held| held.id == *id && held.detach)
        {
            held.pinned = !held.pinned;
            held.open = false;
            held.viewing = false;
        }
        if self.focus.as_ref() == Some(id) {
            self.focus = None;
        }
    }

    /// Give a running block the keyboard where it is drawn; a pinned one opens to take it.
    pub fn focus_on(&mut self, id: &ToolCallId) {
        if let Some(held) = self.surfaces.iter_mut().find(|held| held.id == *id) {
            if held.pinned {
                held.open = true;
            }
            self.focus = Some(id.clone());
        }
    }

    /// Open a pinned block and give it the keyboard, or fold it and give the keyboard back.
    pub fn toggle_docked(&mut self, id: &ToolCallId) {
        let open = self.surfaces.iter().any(|held| held.id == *id && held.open);
        if open {
            self.let_go_of(id);
        } else {
            self.focus_on(id);
        }
    }

    /// Give the keyboard back from `id`, if it has it, and fold it where it was opened to be used.
    fn let_go_of(&mut self, id: &ToolCallId) {
        if self.focus.as_ref() == Some(id) {
            self.let_go();
        } else if let Some(held) = self.surfaces.iter_mut().find(|held| held.id == *id) {
            held.open = false;
        }
    }

    /// Open the list of running blocks, or close it if it is what is showing.
    pub fn press_running(&mut self) {
        if self.pane_titled("running") {
            self.pane = None;
            return;
        }
        let (rows, picks) = self
            .running()
            .map(|held| {
                let at = if held.pinned { "  pinned" } else { "" };
                let row = ratatui::text::Line::from(format!("{}  {}{at}", held.tool, held.about));
                (row, Some(held.id.to_string()))
            })
            .unzip();
        let mut pane = magi_tui::pane::Pane::new("running", rows)
            .selectable(picks)
            .saying("nothing is running");
        pane.first();
        self.pane = Some(pane);
    }

    /// The block picked from the list of running blocks: flashed where it is when it is on screen,
    /// and opened in the float to be used when it is not. Never drawn in two places at once.
    pub fn pick_running(&mut self, id: &str) {
        self.pane = None;
        let Some(held) = self.surfaces.iter().find(|held| held.id.as_str() == id) else {
            return;
        };
        let id = held.id.clone();
        let seen = held.pinned
            || held
                .call
                .as_ref()
                .is_some_and(|call| self.header_in_view(call));
        if seen {
            self.flash = Some((id, std::time::Instant::now()));
            return;
        }
        if let Some(held) = self.surfaces.iter_mut().find(|held| held.id == id) {
            held.viewing = true;
        }
        self.focus = Some(id);
    }

    /// A press on a row of the list of running blocks picks it. Whether it was that list.
    pub fn press_running_row(&mut self, row: u16) -> bool {
        if !self.pane_titled("running") {
            return false;
        }
        let picked = self.pane_rect.and_then(|at| {
            self.pane
                .as_ref()?
                .selected(row.checked_sub(at.y)?)
                .map(ToOwned::to_owned)
        });
        if let Some(id) = picked {
            self.pick_running(&id);
        }
        true
    }

    /// Whether the header of call `call`'s block is on screen in the conversation.
    fn header_in_view(&self, call: &ToolCallId) -> bool {
        self.stack
            .find(call)
            .and_then(|nth| self.stack.block(nth))
            .and_then(|(start, _)| start.checked_sub(self.scrollback.hidden_above()))
            .is_some_and(|into| into < self.live_rows.len())
    }

    /// The surface whose block is lit this frame, while a flash runs; the flash ends when its time
    /// is up.
    pub fn flashing(&mut self) -> Option<ToolCallId> {
        let (id, since) = self.flash.clone()?;
        let elapsed = since.elapsed();
        if elapsed >= FLASH {
            self.flash = None;
            return None;
        }
        (elapsed.as_millis() / BEAT).is_multiple_of(2).then_some(id)
    }

    /// A press on a pin chip in a running block's header in the conversation. Whether it was one.
    pub fn pin_at(&mut self, row: u16, column: u16) -> bool {
        let Some((line, nth)) = self.transcript_row(row) else {
            return false;
        };
        let chip = self
            .stack
            .line(line)
            .and_then(|drawn| chip_under(drawn, column));
        if !matches!(chip, Some(Chip::Pin)) {
            return false;
        }
        match self.surface_of(nth) {
            Some(id) => {
                self.toggle_pin(&id);
                true
            }
            None => false,
        }
    }

    /// A press on a running block's body in the conversation gives it the keyboard. Whether it was.
    pub fn take_focus_at(&mut self, row: u16) -> bool {
        let Some((line, nth)) = self.transcript_row(row) else {
            return false;
        };
        let header = self
            .stack
            .line(line)
            .is_some_and(magi_tui::transcript::is_header);
        match self.surface_of(nth).filter(|_| !header) {
            Some(id) if self.focus.as_ref() != Some(&id) => {
                self.focus_on(&id);
                true
            }
            _ => false,
        }
    }

    /// A press on a pinned block above the prompt: its handle opens or folds it, its pin puts it
    /// back in the conversation, and its body gives it the keyboard. Whether it landed on one.
    pub fn press_dock(&mut self, row: u16, column: u16, width: u16) -> bool {
        let Some((id, at)) = self
            .docked
            .iter()
            .find(|(_, at)| row >= at.y && row < at.y + at.height)
            .cloned()
        else {
            return false;
        };
        if row == at.y {
            let header = self
                .docking()
                .into_iter()
                .find(|(_, _, held)| *held == id)
                .and_then(|(live, open, _)| {
                    magi_tui::transcript::docked(&live, open, width)
                        .into_iter()
                        .next()
                });
            match header.and_then(|header| chip_under(&header, column)) {
                Some(Chip::Fold) => self.toggle_docked(&id),
                Some(Chip::Pin) => self.toggle_pin(&id),
                None => {}
            }
        } else if self.focus.as_ref() != Some(&id) {
            self.focus_on(&id);
        }
        true
    }

    /// Note the pointer over a chip in a pinned block's header, and say whether that changed what
    /// is lit, so a redraw is asked for only when it did.
    pub fn hover_dock(&mut self, row: u16, column: u16, width: u16) -> bool {
        let now = self
            .docked
            .iter()
            .find(|(_, at)| row == at.y)
            .and_then(|(id, _)| {
                let header = self
                    .docking()
                    .into_iter()
                    .find(|(_, _, held)| held == id)
                    .and_then(|(live, open, _)| {
                        magi_tui::transcript::docked(&live, open, width)
                            .into_iter()
                            .next()
                    })?;
                chip_under(&header, column).map(|_| (id.clone(), column))
            });
        let changed = now != self.dock_hover;
        self.dock_hover = now;
        changed
    }

    /// The transcript row a screen row shows, and the block it belongs to.
    fn transcript_row(&self, row: u16) -> Option<(usize, usize)> {
        if !self.live_rows.contains(&row) {
            return None;
        }
        let line = self.scrollback.hidden_above() + usize::from(row - self.live_rows.start);
        let (nth, _) = self.stack.at(line)?;
        Some((line, nth))
    }

    /// The running surface drawing in block `nth`.
    fn surface_of(&self, nth: usize) -> Option<ToolCallId> {
        let call = self.stack.call(nth)?;
        self.running()
            .find(|held| held.call.as_ref() == Some(call))
            .map(|held| held.id.clone())
    }
}

/// A chip in a running block's header.
#[derive(Debug, PartialEq, Eq)]
enum Chip {
    Fold,
    Pin,
}

/// The chip under `column` of a header, if there is one.
fn chip_under(line: &ratatui::text::Line<'_>, column: u16) -> Option<Chip> {
    let mut at = 0_u16;
    for span in &line.spans {
        let wide = u16::try_from(span.content.chars().count()).unwrap_or(0);
        if (at..at.saturating_add(wide)).contains(&column) {
            let has = |glyph: &str| span.content.contains(glyph);
            if has(magi_tui::glyph::pin()) || has(magi_tui::glyph::unpin()) {
                return Some(Chip::Pin);
            }
            if has(magi_tui::glyph::expand()) || has(magi_tui::glyph::collapse()) {
                return Some(Chip::Fold);
            }
            return None;
        }
        at = at.saturating_add(wide);
    }
    None
}

#[cfg(test)]
#[path = "running/tests.rs"]
mod tests;
