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

    /// The tool calls with a question open, so their rows can say so: a row that is waiting on
    /// the person reads the same as one that is merely slow until something marks it.
    #[must_use]
    pub fn waiting_calls(&self) -> Vec<ToolCallId> {
        if self.asks.is_empty() {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool {
                    id, result: None, ..
                } => Some(id),
                _ => None,
            })
            .filter(|call| self.ask_of(call).is_some())
            .cloned()
            .collect()
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

/// What a waiting row says, longest first: whichever fits in the padding the row ends with.
const MARKS: &[&str] = &[
    "◀ waiting on you — click to answer ",
    "◀ waiting — click ",
    "◀ ",
];

/// Say on a tool's row that it is waiting on the person. A row is padded to the full width, so
/// the words take the place of padding rather than following it off the edge of the screen.
pub fn marked(row: &mut ratatui::text::Line<'static>) {
    // The padding may be several spans: whole ones that are nothing but space, and the tail of
    // the one before them.
    let mut padding = 0;
    for span in row.spans.iter().rev() {
        let spaces = span.content.chars().rev().take_while(|c| *c == ' ').count();
        padding += spaces;
        if spaces < span.content.chars().count() {
            break;
        }
    }
    let Some(mark) = MARKS.iter().find(|mark| mark.chars().count() < padding) else {
        return;
    };
    let mut take = mark.chars().count();
    while take > 0 {
        let Some(last) = row.spans.last_mut() else {
            return;
        };
        let held = last.content.chars().count();
        if held <= take {
            take -= held;
            row.spans.pop();
        } else {
            last.content = last
                .content
                .chars()
                .take(held - take)
                .collect::<String>()
                .into();
            take = 0;
        }
    }
    row.spans.push(ratatui::text::Span::styled(
        *mark,
        ratatui::style::Style::default()
            .fg(magi_tui::colour::warning())
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
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
        // Only the calls that are asking are marked, and a finished one never is.
        let waiting: Vec<String> = app
            .waiting_calls()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(waiting, ["c1", "c2", "c3"]);
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

#[cfg(test)]
mod marking {
    use ratatui::text::{Line, Span};

    fn width(row: &Line<'static>) -> usize {
        row.spans.iter().map(|s| s.content.chars().count()).sum()
    }

    #[test]
    fn a_waiting_row_says_so_inside_the_width_it_already_had() {
        // A tool's row is padded out to the edge, so words added after it were drawn off screen.
        let mut row = Line::from(vec![
            Span::raw("  · [ read ] /w/PLAN.md"),
            Span::raw(" ".repeat(47)),
        ]);
        let before = width(&row);
        super::marked(&mut row);
        let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("waiting on you"), "{text}");
        assert_eq!(width(&row), before, "no wider than it was: {text}");
    }

    #[test]
    fn the_row_the_renderer_really_draws_takes_the_words() {
        // Against what is actually drawn, not a row made up here: the padding is its own spans.
        let mut rows = magi_tui::transcript::entry_lines(
            &magi_proto::Entry::Tool {
                id: magi_proto::ToolCallId::new("t1"),
                name: "read".into(),
                args: r#"{"path":"/home/x/PLAN.md"}"#.into(),
                result: None,
                thought_signature: None,
            },
            90,
            magi_tui::transcript::Detail::Preview,
        );
        let row = rows.iter_mut().find(|row| width(row) > 0).expect("a row");
        let before = width(row);
        super::marked(row);
        let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("waiting on you"), "{text}");
        assert!(text.contains("PLAN.md"), "the call is still there: {text}");
        assert_eq!(width(row), before, "{text}");
    }

    #[test]
    fn a_row_with_little_room_says_it_shorter_and_one_with_none_is_left_alone() {
        let mut tight = Line::from(vec![
            Span::raw("  · [ shell ] ls"),
            Span::raw(" ".repeat(6)),
        ]);
        super::marked(&mut tight);
        let text: String = tight.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('◀') && !text.contains("waiting"), "{text}");

        let mut full = Line::from(vec![Span::raw("  · [ shell ] a very long command")]);
        let before = full.clone();
        super::marked(&mut full);
        assert_eq!(full, before);
    }
}
