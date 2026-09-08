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

    /// Open what is known about the project.
    ///
    /// `:graph init` is where the indexing will go. It is named now, and refuses now, because a
    /// command that silently did nothing would be worse than one that says it is not built: the
    /// first is indistinguishable from an index that found nothing.
    pub fn show_graph(&mut self, input: &str, width: u16) {
        let asked = input.split_whitespace().nth(1);
        if asked == Some("init") {
            self.show_notice(
                "graph: indexing is not built yet. When it is, this will read the tree and \
                 record what depends on what."
                    .to_owned(),
            );
            return;
        }
        // The bars are sized against the panel the caller measured, not against a guess: a
        // chart drawn for a width it is not shown at is a chart with the wrong proportions.
        let inside = magi_tui::pane::Pane::area(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width,
            height: 1,
        })
        .width
        .saturating_sub(2) as usize;
        self.pane = Some(
            magi_tui::pane::Pane::new("graph", self.graph.ranked(inside))
                .saying(magi_tui::graph::Graph::empty()),
        );
    }
}
