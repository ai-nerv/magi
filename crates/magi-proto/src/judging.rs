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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Advice {
    pub safe: bool,
    /// The rule it went by, in a word or two: `read-only`, `data-exfiltration`, `destroys-work`.
    #[serde(default)]
    pub rule: String,
    /// One line on what the action does and why that is or is not within what was asked for.
    #[serde(default)]
    pub reason: String,
    /// How sure it is: 1.0 certainly safe, 0.0 certainly not. A model that writes leaves it
    /// empty, and is taken at its word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sure: Option<f64>,
}

impl Advice {
    /// Whether this is too near the middle to act on, which is the person's to settle.
    #[must_use]
    pub fn unsure(&self, band: (f64, f64)) -> bool {
        self.sure
            .is_some_and(|sure| sure >= band.0 && sure <= band.1)
    }
}

/// What the second model is, where there is one. Only one that decides answers with a number,
/// which is what a band can be drawn across.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Nobody is configured for the `safety` role.
    #[default]
    None,
    /// A model that writes: melchior's chat dialects.
    Writes,
    /// A model that decides: melchior's `decisions` dialect.
    Decides,
}

impl Kind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Writes => "writes",
            Self::Decides => "decides",
        }
    }
}

/// Who is asked about what no rule covers, and on what terms. What `:permission` shows and sets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judging {
    pub mode: Mode,
    /// The model in the `safety` role, and what kind it is.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub kind: Kind,
    /// How sure a verdict must be to be acted on: what falls between goes to the person.
    #[serde(default = "Judging::band")]
    pub unsure: (f64, f64),
    /// What `magi.deny` and `magi.ask` name, as a person wrote them.
    #[serde(default)]
    pub denied: Vec<String>,
    #[serde(default)]
    pub always_asked: Vec<String>,
    /// How the second model has been doing this session.
    #[serde(default)]
    pub judged: u32,
    #[serde(default)]
    pub refused: u32,
    #[serde(default)]
    pub in_a_row: u32,
}

impl Judging {
    /// Measured rather than chosen: over 62 labelled commands every dangerous one scored 0.22 or
    /// under and every safe one 0.24 or over.
    #[must_use]
    pub const fn band() -> (f64, f64) {
        (0.2, 0.8)
    }

    /// The band moved by a step, kept apart and inside nought and one.
    #[must_use]
    pub fn widened(self, by: f64) -> (f64, f64) {
        const STEP: f64 = 0.05;
        let (low, high) = self.unsure;
        let low = (low + by * STEP).clamp(0.0, 0.45);
        let high = (high - by * STEP).clamp(0.55, 1.0);
        (
            (low * 100.0).round() / 100.0,
            (high * 100.0).round() / 100.0,
        )
    }
}

impl Default for Judging {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            model: None,
            kind: Kind::default(),
            unsure: Self::band(),
            denied: Vec::new(),
            always_asked: Vec::new(),
            judged: 0,
            refused: 0,
            in_a_row: 0,
        }
    }
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
