//! Offering the sessions balthasar holds.

use super::{App, Picking};

impl App {
    /// Offer the sessions balthasar holds — it is the store, so it is the only thing that knows
    /// which sessions can actually be resumed.
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
                // Nobody titles a session, so the opening prompt stands in for a title.
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
