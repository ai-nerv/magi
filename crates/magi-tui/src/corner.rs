//! What the prompt box wears in its right corner, and what pressing it opens.
//!
//! **A slot, not a feature.** The box wears one small thing down its right-hand side: the number
//! you want while you are deciding what to send, in the place you are already looking when you
//! decide. Today that is usage, and pressing it opens the cost view. Tomorrow it might be
//! something else, and the only things that should have to change are the two lines here that
//! say what it draws and what it opens.
//!
//! **The label and the view are one decision.** A corner that showed one thing and opened another
//! would be a button that lies about itself — you press what you were reading, so what you were
//! reading has to be what you get more of.

/// What the corner is showing.
///
/// One variant today. It is an enum rather than a constant so that adding the second is a variant
/// and two match arms, rather than finding every place the first was assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Corner {
    /// Tokens in and out, and how full the window is. Opens the cost view.
    #[default]
    Cost,
}

impl Corner {
    /// The title of the view this opens.
    ///
    /// Used to tell "this corner's view is already open" from "some other view is", which is what
    /// makes a second press close it rather than reopen it.
    #[must_use]
    pub fn opens(self) -> &'static str {
        match self {
            Self::Cost => "cost",
        }
    }

    /// What it draws, given what the footer knows.
    ///
    /// Empty means "wear nothing": a session with no usage yet has nothing worth three characters
    /// of a screen somebody new is trying to read.
    #[must_use]
    pub fn label(self, data: &crate::footer::FooterData) -> String {
        match self {
            Self::Cost => crate::footer::usage(data),
        }
    }

    /// The same, cut to what will fit.
    ///
    /// The strip is reserved on every row of the box, so anything long here takes the prompt with
    /// it. When it will not fit, each corner says which part of itself matters most — for usage
    /// that is the window, because the totals are a tally and the window is a limit you are
    /// walking towards.
    #[must_use]
    pub fn fitted(self, data: &crate::footer::FooterData, width: u16) -> String {
        let whole = self.label(data);
        if whole.chars().count() <= usize::from(width) / 3 {
            return whole;
        }
        match self {
            Self::Cost => crate::footer::usage(&crate::footer::FooterData {
                input_tokens: 0,
                output_tokens: 0,
                ..data.clone()
            }),
        }
    }
}
