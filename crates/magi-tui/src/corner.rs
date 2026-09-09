//! The prompt box's right-corner slot: it draws one number, and pressing it opens more of that same thing.

/// What the corner is showing. An enum so a second one is a variant plus match arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Corner {
    #[default]
    Cost,
}

impl Corner {
    /// The title of the view this opens, used to tell this corner's view from any other open one.
    #[must_use]
    pub fn opens(self) -> &'static str {
        match self {
            Self::Cost => "cost",
        }
    }

    /// What it draws. Empty means wear nothing.
    #[must_use]
    pub fn label(self, data: &crate::footer::FooterData) -> String {
        match self {
            Self::Cost => crate::footer::usage(data),
        }
    }

    /// The same, cut to what will fit: each corner says which part of itself matters most.
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
