//! melchior's and casper's floats, and the shape all three siblings' floats share.
//!
//! One frame between them — tabs across the heading, the border in that sibling's own colour — and
//! three different questions. balthasar's own view is in `views`, which this is split out of.

use super::App;
use super::views::{CREW, TOOLING, card_width};

impl App {
    /// melchior's float: the run's roster, asked three ways. Every row of it is already here,
    /// pushed for the agents view, so nothing is asked for when it opens.
    pub fn show_crew(&mut self, tab: usize) {
        let mine = self.named.split('/').nth(2);
        let spent: Vec<Vec<(String, magi_proto::Usage)>> = self
            .reachable
            .iter()
            .map(|them| {
                them.spent
                    .iter()
                    .map(|row| (row.model.clone(), row.usage()))
                    .collect()
            })
            .collect();
        let names: Vec<String> = self
            .reachable
            .iter()
            .map(|them| format!("{}/{}", named_role(&them.role), them.id))
            .collect();
        let agents: Vec<magi_tui::crew::Agent<'_>> = self
            .reachable
            .iter()
            .enumerate()
            .map(|(nth, them)| magi_tui::crew::Agent {
                name: &names[nth],
                role: named_role(&them.role),
                here: Some(them.id.as_str()) == mine,
                phase: them.phase.as_deref().unwrap_or("idle"),
                claim: them.claim.as_deref(),
                spent: &spent[nth],
            })
            .collect();
        let drawn = magi_tui::crew::view(
            &magi_tui::crew::Held {
                agents: &agents,
                width: card_width(),
            },
            tab,
        );
        self.pane = Some(self.sibling_pane(CREW, drawn, tab, magi_tui::crew::empty(tab), 0));
    }

    /// Ask the tools program what it offers, once, on a thread of its own: it is a process, and a
    /// float must open now rather than when that process gets round to answering.
    fn ask_for_tools(&mut self) {
        if self.tools.is_some() || self.tools_rx.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let program = self.tools_program.clone();
        std::thread::spawn(move || {
            let _ = sender.send(crate::offered::fetch(&program));
        });
        self.tools_rx = Some(receiver);
    }

    /// Take that answer if it has come, and redraw casper's float if it is open.
    pub fn poll_tools(&mut self) {
        let Some(found) = self.tools_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.tools = Some(found);
        self.tools_rx = None;
        if let Some(tab) = self.pane_tab(TOOLING) {
            self.show_tooling(tab);
        }
    }

    /// Which tab of `title`'s float is showing, when it is the one open.
    #[must_use]
    fn pane_tab(&self, title: &str) -> Option<usize> {
        self.pane
            .as_ref()
            .filter(|open| open.title == title)
            .map(|open| open.tab)
    }

    /// casper's float: what it offers, and what this session reached for.
    pub fn show_tooling(&mut self, tab: usize) {
        self.ask_for_tools();
        let calls = self.tool_calls();
        let drawn = magi_tui::tooling::view(
            &magi_tui::tooling::Held {
                tools: self.tools.as_deref(),
                calls: &calls,
                width: card_width(),
            },
            tab,
        );
        let saying = magi_tui::tooling::empty(tab, self.tools.is_some());
        self.pane = Some(self.sibling_pane(TOOLING, drawn, tab, saying, 2));
    }

    /// How often each tool was called this session, busiest first. Counted off the transcript,
    /// which is the window on what balthasar holds rather than a tally of its own.
    #[must_use]
    fn tool_calls(&self) -> Vec<(String, usize)> {
        let mut counted: std::collections::BTreeMap<&str, usize> =
            std::collections::BTreeMap::new();
        for entry in &self.entries {
            if let magi_proto::Entry::Tool { name, .. } = entry {
                *counted.entry(name.as_str()).or_default() += 1;
            }
        }
        let mut rows: Vec<(String, usize)> = counted
            .into_iter()
            .map(|(name, n)| (name.to_owned(), n))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }

    /// One sibling's float, built the way all three are: tabs across the heading, the border in
    /// that sibling's colour, and the cursor kept where it was.
    pub(super) fn sibling_pane(
        &self,
        title: &'static str,
        drawn: magi_tui::model_card::Rendered,
        tab: usize,
        saying: String,
        nth: usize,
    ) -> magi_tui::pane::Pane {
        let tabs: Vec<String> = match title {
            CREW => magi_tui::crew::TABS
                .iter()
                .map(|t| (*t).to_owned())
                .collect(),
            TOOLING => magi_tui::tooling::TABS
                .iter()
                .map(|t| (*t).to_owned())
                .collect(),
            _ => magi_tui::memory::TABS
                .iter()
                .map(|t| (*t).to_owned())
                .collect(),
        };
        let was = self
            .pane
            .as_ref()
            .filter(|open| open.title == title && open.tab == tab)
            .map(|open| (open.chosen().map(ToOwned::to_owned), open.top));
        let mut pane = magi_tui::pane::Pane::new(title, drawn.rows)
            .selectable(drawn.picks)
            .saying(saying)
            .tinted(Some(magi_tui::footer::hue(nth)))
            .tabbed(tabs, tab);
        match was {
            Some((on, top)) => {
                if !on.is_some_and(|id| pane.point_at(&id)) {
                    pane.first();
                }
                pane.top = top;
            }
            None => pane.first(),
        }
        pane
    }
}

/// A role that was never set reads as `main`, the way the agents view names it.
fn named_role(role: &str) -> &str {
    if role.is_empty() { "main" } else { role }
}
