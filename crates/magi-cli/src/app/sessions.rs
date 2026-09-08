//! Offering the sessions balthasar holds.
//!
//! Split out under THE RULE; the app next door is what this is about.

use super::{App, Picking};

impl App {
    /// Offer the sessions balthasar holds.
    ///
    /// **Asked, not listed.** This read a directory of JSONL journals once, and then read it as a
    /// fallback when balthasar was quiet. Both are gone: balthasar is the store, so it is the only
    /// thing that knows what exists, and a picker built from files offered sessions that could not
    /// be resumed.
    pub fn open_session_picker(&mut self) {
        let found = magi_host::paths::recorded();
        if found.is_empty() {
            self.show_notice(
                "No earlier sessions in this directory. This one is the first.".to_owned(),
            );
            return;
        }

        let choices: Vec<magi_tui::picker::Choice> = found
            .iter()
            .map(|found| magi_tui::picker::Choice {
                // What it was for, which is the only thing anybody recognises a session by.
                // Nobody titles one, so the opening prompt stands in for a title.
                value: if found.title.is_empty() {
                    "(nothing was asked)".to_owned()
                } else {
                    found.title.clone()
                },
                detail: format!("{} entries", found.entries),
                ready: true,
            })
            .collect();
        self.overlay = Some(
            magi_tui::picker::Picker::new("Continue which session?", choices.clone(), None).into(),
        );
        self.picking = Some(Picking::Session {
            rows: choices
                .iter()
                .map(|choice| choice.value.clone())
                .zip(found.into_iter().map(|found| found.id))
                .collect(),
        });
    }
}
