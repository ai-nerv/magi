//! The one thing that opens under the prompt.
//!
//! A completion popup and a selection list were two fields, two heights, two draw calls and two
//! sets of key handling, held apart by a comment saying they never open together. They are one
//! slot: what varies is which rows it holds and what taking a row means.
//!
//! It is drawn *inside* the prompt box rather than beneath it — see [`crate::prompt`] — so the
//! rows carry no background of their own. The box is what says where the menu is.

use crate::complete::Completion;
use crate::picker::Picker;
use ratatui::text::Line;

/// What is open under the prompt.
pub enum Overlay {
    /// A list of choices: a model, a thinking level, a session, a permission.
    Picker(Picker),
    /// The commands or paths that match what is typed.
    Completion(Completion),
}

impl Overlay {
    /// How many rows it wants.
    #[must_use]
    pub fn height(&self) -> u16 {
        match self {
            Self::Picker(picker) => picker.height(),
            Self::Completion(completion) => completion.height(),
        }
    }

    /// Its rows, `width` wide.
    #[must_use]
    pub fn render(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            Self::Picker(picker) => crate::picker::render(picker, width),
            Self::Completion(completion) => crate::complete::render(completion, width),
        }
    }

    /// The list, when that is what this is.
    pub fn picker(&mut self) -> Option<&mut Picker> {
        match self {
            Self::Picker(picker) => Some(picker),
            Self::Completion(_) => None,
        }
    }

    /// The list, to read what it is offering.
    #[must_use]
    pub fn list(&self) -> Option<&Picker> {
        match self {
            Self::Picker(picker) => Some(picker),
            Self::Completion(_) => None,
        }
    }

    /// The popup, when that is what this is.
    pub fn completion(&mut self) -> Option<&mut Completion> {
        match self {
            Self::Completion(completion) => Some(completion),
            Self::Picker(_) => None,
        }
    }

    /// What is open, as one string, for anything that has to notice when it changes.
    ///
    /// The title when there is one, so a permission ask following a model list reads as a second
    /// opening. A popup has no title and answers with the character that opened it, because it
    /// refilters on every keystroke: pressing `/` opens the menu once, and narrowing it to `/mo`
    /// is still that one opening.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Picker(picker) => &picker.title,
            Self::Completion(popup) => match popup.kind {
                crate::complete::Kind::Command => ":",
                crate::complete::Kind::Path => "@",
            },
        }
    }

    /// Whether this is a completion popup.
    #[must_use]
    pub fn is_completion(&self) -> bool {
        matches!(self, Self::Completion(_))
    }

    /// Whether this is a selection list.
    #[must_use]
    pub fn is_picker(&self) -> bool {
        matches!(self, Self::Picker(_))
    }
}

impl From<Picker> for Overlay {
    fn from(picker: Picker) -> Self {
        Self::Picker(picker)
    }
}

impl From<Completion> for Overlay {
    fn from(completion: Completion) -> Self {
        Self::Completion(completion)
    }
}

/// Everything that opens under the prompt says what it is.
#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn a_list_is_known_by_what_it_is_choosing() {
        let picker = Picker::new("model", Vec::new(), None);
        assert_eq!(Overlay::Picker(picker).key(), "model");
    }

    /// A popup, completing `kind`.
    fn popup(kind: crate::complete::Kind) -> Overlay {
        Overlay::Completion(Completion {
            kind,
            candidates: Vec::new(),
            selected: 0,
            typed: String::new(),
            token_start: 0,
        })
    }

    #[test]
    fn the_slash_menu_has_a_key_of_its_own() {
        // It had none, so pressing `/` opened a menu that never landed.
        assert_eq!(popup(crate::complete::Kind::Command).key(), ":");
    }

    #[test]
    fn completing_a_path_is_not_the_same_menu() {
        assert_ne!(
            popup(crate::complete::Kind::Path).key(),
            popup(crate::complete::Kind::Command).key(),
            "@ and / are two menus, and each one opening is its own"
        );
    }
}
