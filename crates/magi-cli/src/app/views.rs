//! Opening the info pane: which view, and what goes in it.

use super::App;

impl App {
    /// Open the timeline of what this session has done, at the newest end.
    pub fn show_trace(&mut self) {
        self.pane = Some(
            magi_tui::pane::Pane::new("trace", self.timeline.lines())
                .saying("nothing has happened yet")
                .following(),
        );
    }

    /// Open what this session has spent, per turn and in total. Tokens, not money: magi does not
    /// know the rates, melchior does.
    pub fn show_cost(&mut self) {
        let turns: Vec<magi_tui::cost::Turn> = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                magi_proto::Entry::Assistant { usage, .. }
                    if usage.prompt_tokens() > 0 || usage.output > 0 =>
                {
                    Some(*usage)
                }
                _ => None,
            })
            .enumerate()
            .map(|(at, usage)| magi_tui::cost::Turn { at: at + 1, usage })
            .collect();
        let model = self.model.as_ref().map(|m| m.name.clone());
        self.pane = Some(
            magi_tui::pane::Pane::new("cost", magi_tui::cost::lines(&turns, model.as_deref()))
                .saying(magi_tui::cost::empty()),
        );
    }

    /// Open what the corner is about; a second press closes it. Closes only when the corner's own
    /// view is showing, so pressing it over some other pane opens the corner's.
    pub fn press_corner(&mut self) {
        if self
            .pane
            .as_ref()
            .is_some_and(|open| open.title == self.corner.opens())
        {
            self.pane = None;
            return;
        }
        match self.corner {
            magi_tui::corner::Corner::Cost => self.show_cost(),
        }
    }
}
