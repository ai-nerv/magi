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

    /// What it draws: for the cost corner, how full the context is. Empty wears nothing.
    #[must_use]
    pub fn label(self, data: &crate::footer::FooterData) -> String {
        match self {
            Self::Cost => crate::footer::context(data),
        }
    }

    /// The same, where it fits: a few columns, so it only goes on a screen too narrow for anything.
    #[must_use]
    pub fn fitted(self, data: &crate::footer::FooterData, width: u16) -> String {
        let said = self.label(data);
        if said.chars().count() <= usize::from(width) / 3 {
            said
        } else {
            String::new()
        }
    }
}
