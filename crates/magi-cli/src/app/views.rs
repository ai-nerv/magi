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

    /// Open the run as a tree of agents, this session marked and whichever one is on screen too.
    /// The roster melchior pushes carries each agent's parent, which is what the tree is drawn from.
    pub fn show_agents(&mut self) {
        let mine = self.named.split('/').nth(2);
        let attached = self.attached.as_ref().map(|them| them.id.as_str());
        let agents: Vec<magi_tui::agents::Agent> = self
            .reachable
            .iter()
            .map(|them| magi_tui::agents::Agent {
                id: them.id.clone(),
                role: if them.role.is_empty() {
                    "main".to_owned()
                } else {
                    them.role.clone()
                },
                parent: them.parent.clone(),
                here: Some(them.id.as_str()) == mine,
                attached: Some(them.id.as_str()) == attached,
                busy: them.busy,
                working_for: them.working_for,
                waiting: them.waiting,
                claim: them.claim.clone(),
            })
            .collect();
        self.pane = Some(
            magi_tui::pane::Pane::new("agents", magi_tui::agents::lines(&agents))
                .saying(magi_tui::agents::empty()),
        );
    }

    /// Open the agents tree, or close it if it is what is showing — the toggle a press on the
    /// footer's `< >` control expects.
    pub fn press_agents(&mut self) {
        if self
            .pane
            .as_ref()
            .is_some_and(|open| open.title == "agents")
        {
            self.pane = None;
        } else {
            self.show_agents();
        }
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
