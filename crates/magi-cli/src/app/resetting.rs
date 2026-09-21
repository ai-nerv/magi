//! `:reset` — putting this directory back to what a first run here would find.
//!
//! What accumulates per directory is kept in several places, and none of them is the configuration:
//! a model switched here is remembered beside the store, and the runs and what they taught are
//! balthasar's. Each is cleared on its own, because "start again" usually means one of them.

use super::{App, Picking};
use magi_tui::picker::{Choice, Picker};

/// What a reset can clear, and what each one costs.
pub const WHAT: [(&str, &str); 4] = [
    (
        "model",
        "the model and thinking level remembered here, so `magi.model` is followed again",
    ),
    (
        "memory",
        "everything this project has been taught — irreversible",
    ),
    (
        "sessions",
        "every run recorded here, and what each one said — irreversible",
    ),
    ("all", "all of the above"),
];

impl App {
    /// Open the list of what can be cleared. `what`, when it names one, puts the question up at
    /// once: typing `:reset memory` is choosing it, not confirming it.
    pub fn open_reset_picker(&mut self, what: Option<&str>) {
        if let Some(named) = what {
            if !WHAT.iter().any(|(name, _)| *name == named) {
                let names: Vec<&str> = WHAT.iter().map(|(name, _)| *name).collect();
                self.show_notice(format!(
                    "`:reset {named}` — it clears one of: {}",
                    names.join(", ")
                ));
                return;
            }
            self.confirm_reset(named);
            return;
        }
        let choices = WHAT
            .iter()
            .map(|(value, detail)| Choice {
                value: (*value).to_owned(),
                detail: (*detail).to_owned(),
                ready: true,
            })
            .collect();
        self.overlay = Some(Picker::new("Reset for this directory", choices, None).into());
        self.picking = Some(Picking::Reset);
    }

    /// Ask before clearing `what`. A separate list rather than the picker's own question, so the
    /// row's own words are on screen beside the answer.
    pub fn confirm_reset(&mut self, what: &str) {
        let detail = WHAT
            .iter()
            .find(|(name, _)| *name == what)
            .map(|(_, detail)| *detail)
            .unwrap_or_default();
        let choices = vec![
            Choice {
                value: "no".to_owned(),
                detail: "leave it as it is".to_owned(),
                ready: true,
            },
            Choice {
                value: "yes".to_owned(),
                detail: detail.to_owned(),
                ready: true,
            },
        ];
        self.overlay =
            Some(Picker::new(format!("Reset {what} for this directory?"), choices, None).into());
        self.picking = Some(Picking::Resetting {
            what: what.to_owned(),
        });
    }

    /// Clear it, and say what went.
    pub fn reset_now(&mut self, what: &str) {
        self.overlay = None;
        self.picking = None;
        let said = match what {
            "model" => forget_model(),
            "memory" => forget_memories(),
            "sessions" => forget_runs(),
            "all" => {
                let each = [forget_model(), forget_memories(), forget_runs()];
                each.join(" ")
            }
            _ => format!("nothing here is called {what}"),
        };
        self.show_notice(said);
    }
}

/// Drop what this directory remembered about which model to use.
fn forget_model() -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return "This directory could not be named.".to_owned();
    };
    let cwd = cwd.display().to_string();
    let held = magi_host::remember::of(&cwd);
    if held.model.is_none() && held.thinking.is_none() {
        return "Nothing was remembered here; `magi.model` was already being followed.".to_owned();
    }
    magi_host::remember::forget(&cwd);
    "Forgot the model remembered here — `magi.model` is followed from the next session.".to_owned()
}

/// Remove every memory this project holds, one at a time: balthasar removes a row only when it is
/// named, which is the guard that stops a whole store going on one wrong keystroke.
fn forget_memories() -> String {
    let ids = crate::config::project_memories();
    if ids.is_empty() {
        return "This project had taught it nothing.".to_owned();
    }
    let (mut gone, mut kept) = (0, 0);
    for id in &ids {
        if magi_host::paths::purge_memory(id).is_ok() {
            gone += 1;
        } else {
            kept += 1;
        }
    }
    match kept {
        0 => format!("Removed {gone} memories."),
        _ => format!("Removed {gone} memories; {kept} would not go."),
    }
}

/// Remove every run recorded here, and what each said.
fn forget_runs() -> String {
    let runs = magi_host::paths::recorded();
    if runs.is_empty() {
        return "No run here had been recorded.".to_owned();
    }
    let (mut gone, mut kept) = (0, 0);
    for run in &runs {
        if magi_host::paths::purge(&run.id).is_ok() {
            gone += 1;
        } else {
            kept += 1;
        }
    }
    match kept {
        0 => format!("Removed {gone} runs."),
        _ => format!("Removed {gone} runs; {kept} would not go."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_named_offers_the_list_and_clears_nothing_yet() {
        let mut app = App::new();
        app.open_reset_picker(None);
        assert_eq!(app.picking, Some(Picking::Reset));
        let offered: Vec<String> = app
            .overlay
            .as_ref()
            .and_then(magi_tui::overlay::Overlay::list)
            .expect("a list")
            .choices
            .iter()
            .map(|choice| choice.value.clone())
            .collect();
        assert_eq!(offered, ["model", "memory", "sessions", "all"]);
    }

    #[test]
    fn naming_one_asks_about_it_rather_than_doing_it() {
        // Typing `:reset memory` is choosing, not confirming: it goes straight to the question,
        // and nothing is cleared until that is answered.
        let mut app = App::new();
        app.open_reset_picker(Some("memory"));
        assert_eq!(
            app.picking,
            Some(Picking::Resetting {
                what: "memory".to_owned()
            })
        );
    }

    #[test]
    fn the_safe_answer_is_the_one_in_front() {
        // A stray Enter on a list that removes a project's memory must not remove it.
        let mut app = App::new();
        app.confirm_reset("all");
        let first = app
            .overlay
            .as_ref()
            .and_then(magi_tui::overlay::Overlay::list)
            .expect("a list")
            .choices
            .first()
            .expect("a row")
            .value
            .clone();
        assert_eq!(first, "no");
    }

    #[test]
    fn a_name_it_does_not_know_says_what_it_does_know() {
        let mut app = App::new();
        app.open_reset_picker(Some("everything"));
        assert_eq!(app.picking, None, "nothing was opened");
        let said = match app.entries.last() {
            Some(magi_proto::Entry::Notice { text }) => text.clone(),
            other => panic!("no notice: {other:?}"),
        };
        for name in ["model", "memory", "sessions"] {
            assert!(said.contains(name), "{said}");
        }
    }

    #[test]
    fn answering_no_leaves_it_alone() {
        let mut app = App::new();
        app.confirm_reset("model");
        assert!(app.chose("no".to_owned()).is_none());
        assert_eq!(app.picking, None, "the question was taken down");
    }
}
