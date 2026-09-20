//! Offering the sessions balthasar holds, and the ones it has put away.

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
        self.offer(
            found,
            "Continue which session?",
            "Put {} away?",
            /* archived */ false,
        );
    }

    /// The runs that have been put away. `:archives` opens it, and it is the only way to see
    /// them: `:resume` leaves them out on purpose.
    pub fn open_archive_picker(&mut self) {
        let found = magi_host::paths::archived();
        if found.is_empty() {
            self.show_notice("Nothing has been put away in this directory.".to_owned());
            return;
        }
        self.offer(
            found,
            "Archived — Del removes one for good",
            "Remove {} for good?",
            /* archived */ true,
        );
    }

    /// Act on the row a question was just answered *yes* for.
    ///
    /// Which list is open is what says what that means, and the two are deliberately different
    /// acts: putting a run away can be undone by removing it from the archive's list, and
    /// removing it cannot be undone at all.
    pub fn forget_row(&mut self, value: &str) {
        let (rows, archived) = match self.picking.as_ref() {
            Some(Picking::Session { rows }) => (rows.clone(), false),
            Some(Picking::Archived { rows }) => (rows.clone(), true),
            _ => return,
        };
        let Some(id) = rows
            .iter()
            .find(|(label, _)| label == value)
            .map(|(_, id)| id.clone())
        else {
            return;
        };
        let said = if archived {
            match magi_host::paths::purge(&id) {
                Ok(()) => format!("Removed “{value}” and everything it held."),
                Err(why) => format!("Could not remove “{value}”: {why}"),
            }
        } else if magi_host::paths::put_away(&id) {
            format!("Put “{value}” away. `:archives` has it.")
        } else {
            format!("Could not put “{value}” away.")
        };
        // Reopened on what is left, so the row that has gone is gone from the list too.
        self.overlay = None;
        self.picking = None;
        if archived {
            self.open_archive_picker();
        } else {
            self.open_session_picker();
        }
        self.show_notice(said);
    }

    /// Both lists: the same rows, the same question shape, a different meaning for Delete.
    fn offer(
        &mut self,
        found: Vec<magi_host::paths::Summary>,
        title: &str,
        question: &str,
        archived: bool,
    ) {
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
            magi_tui::picker::Picker::new(title, choices.clone(), None)
                .askable(question)
                .into(),
        );
        let rows: Vec<(String, String)> = choices
            .iter()
            .map(|choice| choice.value.clone())
            .zip(found.into_iter().map(|found| found.id))
            .collect();
        self.picking = Some(if archived {
            Picking::Archived { rows }
        } else {
            Picking::Session { rows }
        });
    }
}
