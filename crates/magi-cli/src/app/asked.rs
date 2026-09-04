//! A question a tool asked, put on screen.
//!
//! The general form of the permission prompt, and drawn with the same picker: a selection tool, a
//! confirmation and a permission are one shape on screen, and nothing here knows which kind it is
//! looking at. That is the whole point of the mechanism — the list of things that can stop and
//! ask the person is not a list anybody has to extend.

use super::{App, Picking};
use magi_proto::ToolCallId;
use magi_proto::tooling::Answer;

impl App {
    /// Open the picker for a question a tool is waiting on.
    pub(super) fn asked(
        &mut self,
        id: ToolCallId,
        tool: &str,
        question: &str,
        options: Vec<Answer>,
    ) {
        let choices = options
            .iter()
            .map(|answer| magi_tui::picker::Choice {
                value: answer.label.clone(),
                detail: answer.about.clone(),
                ready: true,
            })
            .collect();
        self.overlay = Some(
            magi_tui::picker::Picker::new(format!("{tool}: {question}"), choices, None).into(),
        );
        // The rows are kept as `(label, id)` because the picker that held their order is taken
        // by the keypress that chooses one — so by the time the answer is sent there is nothing
        // left to index, and what a person read is not what the tool asked to get back.
        self.picking = Some(Picking::Asked {
            id,
            rows: options
                .into_iter()
                .map(|answer| (answer.label, answer.id))
                .collect(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(id: &str, label: &str) -> Answer {
        Answer {
            id: id.to_owned(),
            label: label.to_owned(),
            about: String::new(),
        }
    }

    #[test]
    fn a_question_opens_a_picker_labelled_with_what_was_asked() {
        let mut app = App::new();
        app.asked(
            ToolCallId::new("q0"),
            "bash",
            "run a shell command?",
            vec![answer("once", "Allow once"), answer("no", "Deny")],
        );
        let open = app
            .overlay
            .as_mut()
            .and_then(magi_tui::overlay::Overlay::picker)
            .expect("a picker");
        assert!(
            open.title.contains("run a shell command?"),
            "{}",
            open.title
        );
        assert_eq!(open.choices.len(), 2);
        assert_eq!(open.choices[0].value, "Allow once");
    }

    #[test]
    fn the_ids_are_kept_beside_the_labels_that_were_drawn() {
        // What a person reads is not what goes back. Losing the pairing here is how an answer
        // reaches a tool as the wrong choice, or as none.
        let mut app = App::new();
        app.asked(
            ToolCallId::new("q1"),
            "pick",
            "which file?",
            vec![answer("a.rs", "src/a.rs"), answer("b.rs", "src/b.rs")],
        );
        let Some(Picking::Asked { id, rows }) = app.picking else {
            panic!("nothing is being picked");
        };
        assert_eq!(id, ToolCallId::new("q1"));
        assert_eq!(
            rows,
            vec![
                ("src/a.rs".to_owned(), "a.rs".to_owned()),
                ("src/b.rs".to_owned(), "b.rs".to_owned()),
            ]
        );
    }

    #[test]
    fn a_question_with_one_answer_is_still_a_question() {
        // "Understood" is a real thing to offer, and a picker of one row is how it is offered.
        let mut app = App::new();
        app.asked(
            ToolCallId::new("q2"),
            "note",
            "ready?",
            vec![answer("ok", "OK")],
        );
        assert!(app.picking.is_some());
    }
}
