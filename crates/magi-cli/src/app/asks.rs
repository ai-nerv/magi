//! The questions a session has open. Several tools may be asking at once and the screen shows one
//! picker at a time, so they are kept here rather than in the slot a picker occupies: a second
//! question used to replace the first on screen, and the first was then waited on by a turn that
//! nobody could answer.

use super::{App, Picking};
use magi_proto::{Entry, HarnessEvent, ToolCallId};

fn id_of(event: &HarnessEvent) -> Option<&ToolCallId> {
    match event {
        HarnessEvent::PermissionAsked { id, .. } | HarnessEvent::Asked { id, .. } => Some(id),
        _ => None,
    }
}

fn tool_of(event: &HarnessEvent) -> Option<&str> {
    match event {
        HarnessEvent::PermissionAsked { tool, .. } | HarnessEvent::Asked { tool, .. } => Some(tool),
        _ => None,
    }
}

impl App {
    /// A question arrived, or was told again to a screen that has just attached. Kept either way,
    /// and shown unless another question is being answered.
    pub(super) fn ask_arrived(&mut self, event: HarnessEvent) {
        let Some(id) = id_of(&event).cloned() else {
            return;
        };
        if !self.asks.iter().any(|open| id_of(open) == Some(&id)) {
            self.asks.push(event);
        }
        if !self.picking.as_ref().is_some_and(Picking::blocking) {
            self.present(&id);
        }
    }

    /// Put one open question on screen. Whatever was showing stays open and can be come back to.
    pub fn present(&mut self, id: &ToolCallId) -> bool {
        let Some(event) = self
            .asks
            .iter()
            .find(|open| id_of(open) == Some(id))
            .cloned()
        else {
            return false;
        };
        match event {
            HarnessEvent::PermissionAsked {
                id,
                tool,
                action,
                offers,
                ..
            } => {
                let choices = offers
                    .iter()
                    .map(|scope| magi_tui::picker::Choice {
                        value: scope.label(&action),
                        detail: String::new(),
                        ready: true,
                    })
                    .chain(std::iter::once(magi_tui::picker::Choice {
                        value: "no".to_owned(),
                        detail: "refuse, and tell the model".to_owned(),
                        ready: true,
                    }))
                    .collect();
                // The call on its own rows, not in the title. A long command clipped into a
                // heading is clipped in the middle of the very thing being decided about.
                let about = magi_tui::wrap::hard(action.subject(), 60);
                self.overlay = Some(
                    magi_tui::picker::Picker::new(
                        format!("{tool} wants to {}{}", action.verb(), self.others_waiting()),
                        choices,
                        None,
                    )
                    .about(about)
                    .into(),
                );
                self.asking_about = action;
                self.picking = Some(Picking::Permission { id, offers });
            }
            HarnessEvent::Asked {
                id,
                tool,
                question,
                options,
                detail,
                ..
            } => self.asked(id, &tool, &question, options, detail),
            _ => return false,
        }
        true
    }

    /// Said in the title, so one question on screen does not read as the only one there is.
    fn others_waiting(&self) -> String {
        match self.asks.len() {
            0 | 1 => String::new(),
            n => format!(
                "  ({} more waiting — click a tool to answer it first)",
                n - 1
            ),
        }
    }

    /// A question was answered, or refused: forget it, and show the next one still open.
    pub fn ask_settled(&mut self, id: &ToolCallId) {
        self.asks.retain(|open| id_of(open) != Some(id));
        if let Some(next) = self.asks.first().and_then(id_of).cloned() {
            self.present(&next);
        }
    }

    /// The open question a tool call's row stands for. A question carries the tool's name and not
    /// the call's id, so they are paired in order: the nth unanswered call of a tool with the nth
    /// question that tool has open.
    #[must_use]
    pub fn ask_of(&self, call: &ToolCallId) -> Option<ToolCallId> {
        let (name, nth) = self
            .entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Tool { id, name, .. } if id == call => Some(name.clone()),
                _ => None,
            })
            .map(|name| {
                let nth = self
                    .entries
                    .iter()
                    .filter_map(|entry| match entry {
                        Entry::Tool {
                            id,
                            name: other,
                            result: None,
                            ..
                        } if *other == name => Some(id),
                        _ => None,
                    })
                    .position(|id| id == call);
                (name, nth)
            })?;
        self.asks
            .iter()
            .filter(|open| tool_of(open) == Some(name.as_str()))
            .nth(nth?)
            .and_then(id_of)
            .cloned()
    }

    /// A press on the row of a tool that is waiting for an answer brings its question up, whichever
    /// one was showing. Anywhere on the row: the whole line is what is waiting.
    pub fn ask_at(&mut self, row: u16) -> bool {
        if !self.live_rows.contains(&row) {
            return false;
        }
        let line = self.scrollback.hidden_above() + usize::from(row - self.live_rows.start);
        let Some(Some(call)) = self.owners.get(line).cloned() else {
            return false;
        };
        self.ask_of(&call).is_some_and(|ask| self.present(&ask))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{App, Picking};
    use magi_proto::permit::{Action, Scope};
    use magi_proto::{Cursor, Entry, HarnessEvent, ToolCallId};

    fn wants(id: &str, tool: &str, path: &str) -> HarnessEvent {
        HarnessEvent::PermissionAsked {
            cursor: Cursor::ZERO,
            id: ToolCallId::new(id),
            tool: tool.into(),
            action: Action::Read { path: path.into() },
            offers: vec![Scope::Once],
        }
    }

    fn showing(app: &App) -> Option<String> {
        match app.picking.as_ref()? {
            Picking::Permission { id, .. } | Picking::Asked { id, .. } => Some(id.to_string()),
            _ => None,
        }
    }

    #[test]
    fn a_second_question_does_not_take_the_first_ones_place() {
        // Two tools asking at once: the second used to replace the first on screen, and the first
        // was then waited on by a turn with no picker left to answer it from.
        let mut app = App::new();
        app.apply(wants("p0", "read", "/w/PLAN.md"));
        app.apply(wants("p1", "shell", "/w"));
        assert_eq!(
            showing(&app).as_deref(),
            Some("p0"),
            "the first is still the one showing"
        );
        assert_eq!(app.asks.len(), 2, "and the second is kept, not lost");
    }

    #[test]
    fn answering_one_brings_up_the_next() {
        let mut app = App::new();
        app.apply(wants("p0", "read", "/w/PLAN.md"));
        app.apply(wants("p1", "shell", "/w"));
        app.ask_settled(&ToolCallId::new("p0"));
        assert_eq!(showing(&app).as_deref(), Some("p1"));
        app.picking = None;
        app.ask_settled(&ToolCallId::new("p1"));
        assert!(app.asks.is_empty());
    }

    #[test]
    fn a_question_told_again_on_the_way_back_is_one_question() {
        // Stepping onto another agent forgets what this one had open, without answering any of it;
        // coming back, the session says again what is still waiting.
        let mut app = App::new();
        app.apply(wants("p0", "read", "/w/PLAN.md"));
        app.attach_to(None);
        assert!(app.asks.is_empty() && app.picking.is_none());
        app.apply(wants("p0", "read", "/w/PLAN.md"));
        app.apply(wants("p0", "read", "/w/PLAN.md"));
        assert_eq!(app.asks.len(), 1);
        assert_eq!(
            showing(&app).as_deref(),
            Some("p0"),
            "and it is on screen again"
        );
    }

    #[test]
    fn a_tools_row_stands_for_the_question_that_tool_has_open() {
        let mut app = App::new();
        for (id, name) in [("c1", "read"), ("c2", "shell"), ("c3", "read")] {
            app.entries.push(Entry::Tool {
                id: ToolCallId::new(id),
                name: name.into(),
                args: String::new(),
                result: None,
                thought_signature: None,
            });
        }
        app.apply(wants("p0", "read", "/w/a"));
        app.apply(wants("p1", "shell", "/w"));
        app.apply(wants("p2", "read", "/w/b"));
        let ask = |call: &str| app.ask_of(&ToolCallId::new(call)).map(|id| id.to_string());
        assert_eq!(ask("c2").as_deref(), Some("p1"));
        assert_eq!(
            ask("c1").as_deref(),
            Some("p0"),
            "the first read is the first read's"
        );
        assert_eq!(
            ask("c3").as_deref(),
            Some("p2"),
            "and the second is the second's"
        );
        // Taken out of order: the last one first.
        assert!(app.present(&ToolCallId::new("p2")));
        assert_eq!(showing(&app).as_deref(), Some("p2"));
        assert_eq!(
            app.asks.len(),
            3,
            "the one it replaced on screen is still open"
        );
    }
}
