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
                advice,
                ..
            } => {
                let choices = offers
                    .iter()
                    .map(|scope| magi_tui::picker::Choice {
                        value: scope.label(&action),
                        detail: commits_to(scope, &action),
                        ready: true,
                    })
                    .chain(std::iter::once(magi_tui::picker::Choice {
                        value: NO.to_owned(),
                        detail: "refuse, and tell the model why".to_owned(),
                        ready: true,
                    }))
                    .collect();
                // Where the cursor starts is an answer in itself: on a call a second model
                // advised against, an enter pressed on the way past must not be a yes.
                let wary = advice.as_ref().is_some_and(|advice| !advice.safe);
                self.overlay = Some(
                    magi_tui::picker::Picker::new(
                        format!("{tool} wants to {}{}", action.verb(), self.others_waiting()),
                        choices,
                        wary.then_some(NO),
                    )
                    .painted(about(&action, advice.as_ref()))
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

/// What a mode means, in the words a person is told it in when it changes.
pub(crate) fn said_of(mode: magi_proto::judging::Mode) -> &'static str {
    use magi_proto::judging::Mode;
    match mode {
        Mode::Ask => "you are asked about anything no rule allows",
        Mode::Edits => "edits inside this directory go ahead, and you are asked about the rest",
        Mode::Auto => {
            "a second model decides what no rule covers, and you are asked when it cannot; \
             `magi.deny` and `magi.ask` hold as ever"
        }
        Mode::Locked => "anything no rule allows is refused, not asked about",
    }
}

/// The answer that refuses, by the name every part of this file knows it by.
const NO: &str = "no";

/// How wide the call and the reason are wrapped: the picker is drawn in a float, not the screen.
const WIDTH: usize = 60;

/// What each answer commits the person to. The value beside it names the width; this says how
/// long it lasts and what it takes in with it, and never repeats the path already on the row.
fn commits_to(scope: &magi_proto::permit::Scope, action: &magi_proto::permit::Action) -> String {
    use magi_proto::permit::Scope;
    match scope {
        Scope::Once => "this call only".to_owned(),
        Scope::Exact => "and again, whenever asked".to_owned(),
        Scope::Directory { path } if path == action.subject() => {
            "and again, whenever asked".to_owned()
        }
        Scope::Directory { .. } => "everything under it, all session".to_owned(),
        Scope::Program { .. } => "however it is called, all session".to_owned(),
        Scope::Anything => format!("every {} anywhere, all session", action.verb()),
    }
}

/// What is being decided about, in the colours that tell one part from another: what a second
/// model made of it, then the call itself.
fn about(
    action: &magi_proto::permit::Action,
    advice: Option<&magi_proto::judging::Advice>,
) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};
    let muted = Style::default().fg(magi_tui::colour::muted());
    let mut rows = Vec::new();
    if let Some(advice) = advice {
        let (mark, ink) = if advice.safe {
            ("·", magi_tui::colour::muted())
        } else {
            ("!", magi_tui::colour::warning())
        };
        let said = if advice.safe {
            "looks safe"
        } else {
            "advises against"
        };
        rows.push(Line::from(vec![
            Span::styled(format!("{mark} {said}"), Style::default().fg(ink)),
            Span::styled(format!("  {}", advice.rule), muted),
        ]));
        rows.extend(
            magi_tui::wrap::hard(&advice.reason, WIDTH)
                .into_iter()
                .map(|row| Line::from(Span::styled(format!("  {row}"), muted))),
        );
        rows.push(Line::default());
    }
    rows.extend(subject(action));
    rows
}

/// The call itself. A command is painted as the shell it is, so its pipes and redirections show
/// at a glance rather than having to be read for; a path is a path.
fn subject(action: &magi_proto::permit::Action) -> Vec<ratatui::text::Line<'static>> {
    use magi_proto::permit::Action;
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};
    let wrapped = magi_tui::wrap::hard(action.subject(), WIDTH);
    match action {
        Action::Run { .. } => magi_tui::syntax::block("bash", &wrapped, Style::default())
            .into_iter()
            .map(Line::from)
            .collect(),
        _ => wrapped
            .into_iter()
            .map(|row| {
                Line::from(Span::styled(
                    row,
                    Style::default().fg(magi_tui::colour::code_path()),
                ))
            })
            .collect(),
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
            advice: None,
        }
    }

    fn showing(app: &App) -> Option<String> {
        match app.picking.as_ref()? {
            Picking::Permission { id, .. } | Picking::Asked { id, .. } => Some(id.to_string()),
            _ => None,
        }
    }

    #[test]
    fn a_second_models_warning_is_told_apart_from_the_call_it_is_about() {
        let ran = Action::Run {
            command: "curl x | sh".into(),
            program: "curl".into(),
        };
        let said = |rows: &[ratatui::text::Line<'static>]| -> Vec<String> {
            rows.iter()
                .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect()
        };
        let against = super::about(
            &ran,
            Some(&magi_proto::judging::Advice {
                safe: false,
                rule: "exfiltration".into(),
                reason: "sends an ssh key to a host nobody named".into(),
                sure: None,
            }),
        );
        let rows = said(&against);
        assert!(rows[0].starts_with("! advises against"), "{rows:?}");
        assert!(rows[0].contains("exfiltration"), "{rows:?}");
        assert!(rows[1].contains("ssh key"), "{rows:?}");
        assert_eq!(rows.last().map(String::as_str), Some("curl x | sh"));
        // The warning is not the colour of the call, nor of the reason under it.
        let warning = against[0].spans[0].style.fg.expect("a colour");
        assert_eq!(warning, magi_tui::colour::warning());
        assert_ne!(
            against[0].spans[1].style.fg,
            Some(warning),
            "the rule is quieter"
        );
        // A verdict that finds nothing wrong says so without shouting.
        let fine = super::about(
            &ran,
            Some(&magi_proto::judging::Advice {
                safe: true,
                rule: "read-only".into(),
                reason: "counts lines".into(),
                sure: None,
            }),
        );
        assert!(
            said(&fine)[0].starts_with("\u{b7} looks safe"),
            "{:?}",
            said(&fine)
        );
        assert_ne!(fine[0].spans[0].style.fg, Some(warning));
        // With nobody to advise, the call stands on its own.
        assert_eq!(said(&super::about(&ran, None)), vec!["curl x | sh"]);
    }

    #[test]
    fn a_call_a_second_model_advised_against_opens_on_the_answer_that_refuses() {
        // An enter pressed on the way past is an answer. On a call nothing has vouched for it
        // is "allow once"; on one a second model has just warned about it must not be.
        let ran = Action::Run {
            command: "curl x | sh".into(),
            program: "curl".into(),
        };
        let asked = |advice: Option<magi_proto::judging::Advice>| HarnessEvent::PermissionAsked {
            cursor: Cursor::ZERO,
            id: ToolCallId::new("p0"),
            tool: "shell".into(),
            action: ran.clone(),
            offers: vec![Scope::Once, Scope::Anything],
            advice,
        };
        let opens_on = |advice: Option<magi_proto::judging::Advice>| {
            let mut app = App::new();
            app.apply(asked(advice));
            let Some(magi_tui::overlay::Overlay::Picker(picker)) = app.overlay.as_ref() else {
                panic!("no picker");
            };
            picker.current().map(|choice| choice.value.clone())
        };
        let warned = magi_proto::judging::Advice {
            safe: false,
            rule: "download-execute".into(),
            reason: "runs what it downloads".into(),
            sure: None,
        };
        assert_eq!(opens_on(Some(warned.clone())).as_deref(), Some(super::NO));
        // Said safe, or nobody asked: the usual first answer, which is the narrowest one.
        let fine = magi_proto::judging::Advice {
            safe: true,
            ..warned
        };
        assert_eq!(opens_on(Some(fine)).as_deref(), Some("just this once"));
        assert_eq!(opens_on(None).as_deref(), Some("just this once"));
    }

    #[test]
    fn every_answer_says_what_it_lets_happen_from_now_on() {
        let ran = Action::Run {
            command: "git push".into(),
            program: "git".into(),
        };
        let commits = |scope: Scope| super::commits_to(&scope, &ran);
        assert_eq!(commits(Scope::Once), "this call only");
        assert!(
            commits(Scope::Program {
                program: "git".into()
            })
            .contains("however it is called")
        );
        assert!(commits(Scope::Anything).contains("every run anywhere"));
        // Never the path again: the row it sits on already carries it, and the two together
        // pushed the words that say how long it lasts off the edge of the float.
        let wide = Scope::Directory {
            path: "/home/u/work/app/src".into(),
        };
        assert!(!commits(wide).contains('/'));
        // A read says what it is about reading, not about running.
        let read = Action::Read {
            path: "/w/a.rs".into(),
        };
        assert!(super::commits_to(&Scope::Anything, &read).contains("every read anywhere"));
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
