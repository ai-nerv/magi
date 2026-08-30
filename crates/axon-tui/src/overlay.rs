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
