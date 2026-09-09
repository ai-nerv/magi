//! The one slot that opens under the prompt: a picker or a completion popup, never both. Drawn
//! *inside* the prompt box (see [`crate::prompt`]), so the rows carry no background of their own.

use crate::complete::Completion;
use crate::picker::Picker;
use ratatui::text::Line;

/// What is open under the prompt.
pub enum Overlay {
    Picker(Picker),
    Completion(Completion),
}

impl Overlay {
    #[must_use]
    pub fn height(&self) -> u16 {
        match self {
            Self::Picker(picker) => picker.height(),
            Self::Completion(completion) => completion.height(),
        }
    }

    #[must_use]
    pub fn render(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            Self::Picker(picker) => crate::picker::render(picker, width),
            Self::Completion(completion) => crate::complete::render(completion, width),
        }
    }

    pub fn picker(&mut self) -> Option<&mut Picker> {
        match self {
            Self::Picker(picker) => Some(picker),
            Self::Completion(_) => None,
        }
    }

    #[must_use]
    pub fn list(&self) -> Option<&Picker> {
        match self {
            Self::Picker(picker) => Some(picker),
            Self::Completion(_) => None,
        }
    }

    pub fn completion(&mut self) -> Option<&mut Completion> {
        match self {
            Self::Completion(completion) => Some(completion),
            Self::Picker(_) => None,
        }
    }

    /// A stable identity for what is open: a picker's title, or the character that opened a popup.
    /// A popup refilters on every keystroke and stays the same opening while it narrows.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Picker(picker) => &picker.title,
            Self::Completion(popup) => match popup.kind {
                crate::complete::Kind::Command => ":",
                crate::complete::Kind::Path => "@",
                crate::complete::Kind::Instance => "$",
                crate::complete::Kind::Skill => "/",
            },
        }
    }

    #[must_use]
    pub fn is_completion(&self) -> bool {
        matches!(self, Self::Completion(_))
    }

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
