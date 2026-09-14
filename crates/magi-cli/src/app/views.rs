//! Opening the info pane: which view, and what goes in it.

use super::App;

/// What melchior said about a model, by the model's name.
pub type Answered = (String, Result<magi_tui::model_card::Details, String>);

/// How wide the model card's charts may be: the float's width on this terminal, less its border,
/// its padding and the list's gutter.
fn card_width() -> u16 {
    let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));
    let float = magi_tui::pane::Pane::area(ratatui::layout::Rect::new(0, 0, width, height));
    float.width.saturating_sub(6)
}

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
        // One run's tree, not every session that shares the project. The run to show is that of
        // whoever is on screen — the attached peer, or this session found in the roster. `None`
        // (an older melchior that does not report the run) falls back to showing the lot.
        let focus = self
            .attached
            .as_ref()
            .and_then(|them| them.session.clone())
            .or_else(|| {
                self.reachable
                    .iter()
                    .find(|them| Some(them.id.as_str()) == mine)
                    .and_then(|me| me.session.clone())
            });
        let agents: Vec<magi_tui::agents::Agent> = self
            .reachable
            .iter()
            .filter(|them| match &focus {
                Some(run) => them.session.as_deref() == Some(run.as_str()),
                None => true,
            })
            .map(|them| {
                let role = if them.role.is_empty() {
                    "main"
                } else {
                    them.role.as_str()
                };
                (them, role)
            })
            .map(|(them, role)| magi_tui::agents::Agent {
                id: them.id.clone(),
                role: role.to_owned(),
                about: self.about.get(role).cloned(),
                parent: them.parent.clone(),
                here: Some(them.id.as_str()) == mine,
                attached: Some(them.id.as_str()) == attached,
                phase: them
                    .phase
                    .as_deref()
                    .and_then(magi_proto::Phase::read)
                    // An older melchior sends no phase but does send `busy`: read that so the
                    // panel is never wrong, only less precise.
                    .unwrap_or(if them.busy {
                        magi_proto::Phase::Working
                    } else {
                        magi_proto::Phase::Idle
                    }),
                cause: them.cause.clone(),
                working_for: them.working_for,
                waiting: them.waiting,
                claim: them.claim.clone(),
            })
            .collect();
        // The cursor stays on whoever it was on across a rebuild; a fresh view starts on the agent
        // the screen is showing.
        let on = self
            .pane
            .as_ref()
            .filter(|open| open.title == "agents")
            .and_then(|open| open.chosen().map(ToOwned::to_owned))
            .or_else(|| attached.map(ToOwned::to_owned))
            .or_else(|| mine.map(ToOwned::to_owned));
        let rendered = magi_tui::agents::view(&agents, &self.folded);
        let mut pane = magi_tui::pane::Pane::new("agents", rendered.rows)
            .selectable(rendered.picks)
            .saying(magi_tui::agents::empty());
        if !on.is_some_and(|id| pane.point_at(&id)) {
            pane.first();
        }
        self.pane = Some(pane);
    }

    /// Shut the branch under the agents view's cursor, or open it. On a leaf, or a branch already
    /// that way, go to its parent or its first child instead: a tree walked from the keyboard.
    pub fn fold_agent(&mut self, open: bool) {
        let Some(pane) = self.pane.as_mut().filter(|pane| pane.title == "agents") else {
            return;
        };
        let Some(id) = pane.chosen().map(ToOwned::to_owned) else {
            return;
        };
        let parent = self
            .reachable
            .iter()
            .find(|them| them.id == id)
            .and_then(|them| them.parent.clone());
        let branch = self
            .reachable
            .iter()
            .any(|them| them.parent.as_deref() == Some(id.as_str()));
        if open {
            if !self.folded.remove(&id) {
                if branch {
                    pane.step(true);
                }
                return;
            }
        } else if !branch || !self.folded.insert(id) {
            if let Some(up) = parent {
                pane.point_at(&up);
            }
            return;
        }
        self.show_agents();
    }

    /// Open the model's card: what it is, its settings, and this session's spend. Redrawn after a
    /// change with the cursor on the row it was on.
    pub fn show_model(&mut self) {
        let turns: Vec<magi_proto::Usage> = self
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
            .collect();
        let name = self.model.as_ref().map_or_else(
            || magi_tui::glyph::no_model().to_owned(),
            |model| model.name.clone(),
        );
        let on = self
            .pane
            .as_ref()
            .filter(|open| open.title == "model")
            .and_then(|open| open.chosen().map(ToOwned::to_owned));
        // Asked once per model, on its own thread; drawn as asking until it answers.
        let known_here = self
            .details
            .as_ref()
            .is_some_and(|(model, _)| *model == name);
        if !known_here && self.details_rx.is_none() && self.model.is_some() {
            self.ask_for_details(&name);
        }
        let details = match self.details.as_ref().filter(|(model, _)| *model == name) {
            Some((_, Ok(found))) => magi_tui::model_card::Known::Found(found),
            Some((_, Err(why))) => magi_tui::model_card::Known::Missing(why),
            None => magi_tui::model_card::Known::Asking,
        };
        let drawn = magi_tui::model_card::view(&magi_tui::model_card::Card {
            model: &name,
            context_window: self.model.as_ref().map_or(0, |model| model.context_window),
            reasons: self.model_reasons,
            thinking: &self.thinking,
            provider: self.provider.as_deref(),
            turns: &turns,
            details,
            width: card_width(),
        });
        let mut pane = magi_tui::pane::Pane::new("model", drawn.rows).selectable(drawn.picks);
        if !on.is_some_and(|id| pane.point_at(&id)) {
            pane.first();
        }
        self.pane = Some(pane);
    }

    /// Ask melchior what the provider publishes about `model`, on a thread: a network call must not
    /// hold the screen. The answer is picked up by [`App::poll_details`].
    fn ask_for_details(&mut self, model: &str) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let program = self.mind.clone();
        let model = model.to_owned();
        std::thread::spawn(move || {
            let found = crate::details::fetch(&program, &model);
            let _ = sender.send((model, found));
        });
        self.details_rx = Some(receiver);
    }

    /// Take an answer about a model if one has come, and redraw its card if it is open.
    pub fn poll_details(&mut self) {
        let Some(answer) = self.details_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.details = Some(answer);
        self.details_rx = None;
        if self.pane_titled("model") {
            self.show_model();
        }
    }

    /// Whether the float open is the view titled `title`.
    #[must_use]
    pub fn pane_titled(&self, title: &str) -> bool {
        self.pane.as_ref().is_some_and(|open| open.title == title)
    }

    /// Open the model's card, or close it if it is what is showing.
    pub fn press_model(&mut self) {
        if self.pane.as_ref().is_some_and(|open| open.title == "model") {
            self.pane = None;
        } else {
            self.show_model();
        }
    }

    /// Step the thinking level on the model's card, and say what to send: `None` at either end, off
    /// the thinking row, or for a model that does not reason.
    pub fn adjust_model(&mut self, forward: bool) -> Option<magi_proto::UiCommand> {
        let on_thinking = self
            .pane
            .as_ref()
            .is_some_and(|open| open.chosen() == Some("thinking"));
        if !on_thinking || !self.model_reasons {
            return None;
        }
        let levels = magi_tui::model_card::LEVELS;
        let now = levels.iter().position(|l| *l == self.thinking).unwrap_or(0);
        let next = if forward {
            (now + 1).min(levels.len() - 1)
        } else {
            now.saturating_sub(1)
        };
        (next != now).then(|| self.set_thinking_here(levels[next]))
    }

    /// Take the row the card's cursor is on: the thinking level steps up and wraps, and the model
    /// row opens the model list in the card's place.
    pub fn choose_on_model(&mut self, id: &str) -> Option<magi_proto::UiCommand> {
        match id {
            "switch" => {
                self.pane = None;
                self.open_model_picker();
                None
            }
            "thinking" if self.model_reasons => {
                let levels = magi_tui::model_card::LEVELS;
                let now = levels.iter().position(|l| *l == self.thinking).unwrap_or(0);
                Some(self.set_thinking_here(levels[(now + 1) % levels.len()]))
            }
            // A provider row, or the router's own: marked at once, then asked of the session.
            _ if id.starts_with("provider:") => {
                let tag = id.trim_start_matches("provider:");
                let provider = (!tag.is_empty()).then(|| tag.to_owned());
                self.provider.clone_from(&provider);
                self.show_model();
                Some(magi_proto::UiCommand::SetProvider { provider })
            }
            _ => None,
        }
    }

    /// Show `level` on the card at once, rather than after the session says so, and say what to send.
    fn set_thinking_here(&mut self, level: &str) -> magi_proto::UiCommand {
        level.clone_into(&mut self.thinking);
        self.show_model();
        magi_proto::UiCommand::SetThinking {
            level: level.to_owned(),
        }
    }

    /// Open the agents tree, or close it if it is what is showing — the toggle a press on the
    /// footer name expects.
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

    /// Open a sibling's menu from its footer dot, the same float the agents and cost views use; a
    /// second press closes it. Empty until each sibling has something to offer there.
    pub fn press_sibling(&mut self, nth: usize) {
        let Some((_, name)) = magi_tui::footer::SIBLINGS.get(nth) else {
            return;
        };
        if self.pane.as_ref().is_some_and(|open| open.title == *name) {
            self.pane = None;
            return;
        }
        self.pane = Some(magi_tui::pane::Pane::new(*name, Vec::new()).saying("nothing here yet"));
    }
}
