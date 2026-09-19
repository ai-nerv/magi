//! Who decides what no rule covers, and what a second model said about it.

use serde::{Deserialize, Serialize};

/// Who is asked about an action no standing rule covers. Rules come first in every mode: what
/// `magi.deny` names never runs, and what `magi.ask` names always reaches the person.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The person, every time.
    #[default]
    Ask,
    /// The person, except for writes inside the session's directory.
    Edits,
    /// A second model, shown what the person asked for and the action, never a tool's output.
    Auto,
    /// Nobody: what would have been asked is refused. For a run nobody is watching.
    Locked,
}

impl Mode {
    pub const ALL: [Self; 4] = [Self::Ask, Self::Edits, Self::Auto, Self::Locked];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edits => "edits",
            Self::Auto => "auto",
            Self::Locked => "locked",
        }
    }

    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.name() == name.trim())
    }

    /// The next one a key cycles to. `locked` is chosen on purpose, never cycled into.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Ask => Self::Edits,
            Self::Edits => Self::Auto,
            Self::Auto | Self::Locked => Self::Ask,
        }
    }
}

/// What the second model said of one action: shown in `ask`, acted on in `auto`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advice {
    pub safe: bool,
    /// The rule it went by, in a word or two: `read-only`, `data-exfiltration`, `destroys-work`.
    #[serde(default)]
    pub rule: String,
    /// One line on what the action does and why that is or is not within what was asked for.
    #[serde(default)]
    pub reason: String,
}

/// What `magi.ask` and `magi.deny` said, which no mode overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    pub ask: Vec<crate::permit::Grant>,
    pub deny: Vec<crate::permit::Grant>,
}

#[cfg(test)]
mod tests {
    use super::Mode;

    #[test]
    fn a_mode_is_named_as_it_is_written_in_a_configuration() {
        for mode in Mode::ALL {
            assert_eq!(Mode::named(mode.name()), Some(mode));
        }
        assert_eq!(Mode::named("yolo"), None);
    }

    #[test]
    fn cycling_never_lands_on_locked_and_always_comes_back_to_ask() {
        let mut mode = Mode::Ask;
        for _ in 0..3 {
            mode = mode.next();
            assert_ne!(mode, Mode::Locked);
        }
        assert_eq!(mode, Mode::Ask);
        assert_eq!(Mode::Locked.next(), Mode::Ask);
    }
}
