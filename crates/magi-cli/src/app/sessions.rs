//! Offering the sessions recorded in this directory.
//!
//! Split out under THE RULE; the app next door is what this is about.

use super::{App, Picking};

impl App {
    /// Offer the sessions recorded in this directory.
    ///
    /// Read here rather than asked of the daemon: the journals are files on this machine, this
    /// process is on the same machine, and a round trip to be told what a directory listing says
    /// would be a protocol message that earns nothing.
    pub fn open_session_picker(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let dir = magi_host::paths::sessions_dir();
        // balthasar first: it is the store, so a directory of journals is either absent or
        // stale, and a picker built from stale files offers sessions that cannot be resumed.
        let found = magi_host::paths::recorded()
            .filter(|found| !found.is_empty())
            .unwrap_or_else(|| magi_host::paths::summaries(&dir, &cwd.display().to_string()));
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
