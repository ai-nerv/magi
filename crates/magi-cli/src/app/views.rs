//! Opening the info pane: which view, and what goes in it.
//!
//! Split out under THE RULE; the app these hang off is next door. Together they are the whole
//! of what a view costs — a function that returns rows and a line in the command table. That is
//! the point of the pane being one primitive rather than one panel per thing worth showing.

use super::App;

impl App {
    /// Open the timeline of what this session has done.
    ///
    /// Opens at the newest end, because that is what somebody typing `:trace` is asking about —
    /// a view that opened at the first thing that ever happened would need scrolling before it
    /// answered anything.
    pub fn show_trace(&mut self) {
        self.pane = Some(
            magi_tui::pane::Pane::new("trace", self.timeline.lines())
                .saying("nothing has happened yet")
                .following(),
        );
    }

    /// Open what this session has spent.
    ///
    /// Per turn and in total, in the four counters a provider bills separately. No money: magi
    /// does not know the rates, melchior does, and a guess printed here would go stale the day a
    /// provider changed one -- see `magi_tui::cost`.
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

    /// Open what the corner is about, or close it if it is already open.
    ///
    /// **A second press closes it.** The corner is a control, and a control that only ever opens
    /// is one you have to reach for the keyboard to undo — which is the opposite of why it is a
    /// button. Closing only when *its own* view is showing: pressing the corner while some other
    /// pane is up should get you the corner's, not nothing.
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
