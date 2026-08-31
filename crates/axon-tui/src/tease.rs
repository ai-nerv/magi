//! The empty prompt, typing to itself.
//!
//! A placeholder is read once and then it is furniture. This is the other way round: the box
//! sits with a plain opener until you have left it alone long enough to be looking elsewhere,
//! and then it writes a line, thinks better of one word, deletes that word and puts another in.
//!
//! **The correction is performed, not drawn.** An earlier version struck the word out with
//! `CROSSED_OUT` and left both halves on screen. Watching it happen is the joke; a line that
//! arrives already corrected is a line with punctuation in it.
//!
//! It stops the instant you touch the keyboard, and starts its wait over.

use std::time::{Duration, Instant};

/// What the box is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// Showing something and not moving. The instant is when that stops.
    Resting(Instant),
    /// Taking back what is there, a character at a time.
    Erasing,
    /// Writing the line as first thought, including the word it will regret.
    Writing,
    /// The regrettable word is on screen and being read. The instant is when the doubt lands.
    Doubting(Instant),
    /// Taking that word back.
    Unwriting,
    /// Writing what it meant.
    Correcting,
}

/// The empty prompt's performance.
pub struct Tease {
    /// What the box shows this frame.
    shown: String,
    phase: Phase,
    /// Everything before the word that gets replaced.
    prefix: String,
    /// The word it writes and then takes back.
    struck: String,
    /// What it puts there instead.
    after: String,
    /// Which line of the list is being performed.
    line: usize,
    /// When the last character moved, so the pace is time and not frame rate.
    stepped: Instant,
}

impl Tease {
    /// A box showing `opener`, with nothing to do for a while.
    #[must_use]
    pub fn new(opener: &str) -> Self {
        let now = Instant::now();
        Self {
            shown: opener.to_owned(),
            phase: Phase::Resting(now + patience()),
            prefix: String::new(),
            struck: String::new(),
            after: String::new(),
            line: 0,
            stepped: now,
        }
    }

    /// What to draw.
    #[must_use]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// Somebody is at the keyboard. Put the opener back and start waiting again.
    ///
    /// Mid-word if that is where it was: a performance that insisted on finishing after you had
    /// started typing would be arguing with you.
    pub fn interrupt(&mut self, opener: &str) {
        self.shown = opener.to_owned();
        self.phase = Phase::Resting(Instant::now() + patience());
        self.stepped = Instant::now();
    }

    /// Move it on if enough time has passed. Says whether anything changed.
    ///
    /// `lines` is the list to draw the next line from, and it is passed rather than held so a
    /// configuration reloaded under a running UI is read rather than remembered.
    pub fn advance(&mut self, lines: &[String]) -> bool {
        if patience().is_zero() || lines.is_empty() {
            return false;
        }
        let now = Instant::now();
        match self.phase.clone() {
            Phase::Resting(until) => {
                if now < until {
                    return false;
                }
                self.line = crate::pick::another(self.line, lines.len());
                let (prefix, struck, after) = split(&lines[self.line]);
                self.prefix = prefix;
                self.struck = struck;
                self.after = after;
                self.phase = Phase::Erasing;
                self.stepped = now;
                true
            }
            Phase::Doubting(until) => {
                if now < until {
                    return false;
                }
                self.phase = Phase::Unwriting;
                self.stepped = now;
                true
            }
            _ => self.step(now),
        }
    }

    /// One character, if the pace allows it.
    fn step(&mut self, now: Instant) -> bool {
        if now.duration_since(self.stepped) < pace() {
            return false;
        }
        self.stepped = now;
        match self.phase {
            Phase::Erasing => {
                if self.shown.pop().is_none() {
                    self.phase = Phase::Writing;
                }
            }
            Phase::Writing => {
                let target = format!("{}{}", self.prefix, self.struck);
                if !grow(&mut self.shown, &target) {
                    self.phase = Phase::Doubting(now + doubt());
                }
            }
            Phase::Unwriting => {
                if self.shown.chars().count() <= self.prefix.chars().count() {
                    self.phase = Phase::Correcting;
                } else {
                    self.shown.pop();
                }
            }
            Phase::Correcting => {
                let target = format!("{}{}", self.prefix, self.after);
                if !grow(&mut self.shown, &target) {
                    self.phase = Phase::Resting(now + patience());
                }
            }
            Phase::Resting(_) | Phase::Doubting(_) => {}
        }
        true
    }
}

/// Add the next character of `target` to `shown`. False when there is none left.
fn grow(shown: &mut String, target: &str) -> bool {
    let Some(next) = target.chars().nth(shown.chars().count()) else {
        return false;
    };
    shown.push(next);
    true
}

/// Split a line into what stays, what is taken back, and what replaces it.
///
/// `a ~~b~~ c` is "write `a b`, take back `b`, write `c`". A line with no marker is written and
/// left alone, which is how a plain line can sit in the same list.
#[must_use]
pub fn split(line: &str) -> (String, String, String) {
    let Some((prefix, rest)) = line.split_once("~~") else {
        return (line.to_owned(), String::new(), String::new());
    };
    let Some((struck, after)) = rest.split_once("~~") else {
        return (line.to_owned(), String::new(), String::new());
    };
    (
        prefix.to_owned(),
        struck.to_owned(),
        after.trim_start().to_owned(),
    )
}

/// How long the box is left alone before it starts.
fn patience() -> Duration {
    Duration::from_millis(crate::metric::tease_after_ms())
}

/// How long one character takes.
fn pace() -> Duration {
    Duration::from_millis(crate::metric::tease_step_ms().max(1))
}

/// How long the regrettable word sits there being read.
fn doubt() -> Duration {
    Duration::from_millis(crate::metric::tease_doubt_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_splits_into_what_stays_what_goes_and_what_replaces_it() {
        let (prefix, struck, after) = split("the scaffolding is ~~temporary~~ the building");
        assert_eq!(prefix, "the scaffolding is ");
        assert_eq!(struck, "temporary");
        assert_eq!(after, "the building");
    }

    #[test]
    fn a_line_with_no_marker_is_written_and_left_alone() {
        // So a plain opener can sit in the same list without a special case.
        let (prefix, struck, after) = split("let's build something");
        assert_eq!(prefix, "let's build something");
        assert!(struck.is_empty() && after.is_empty());
    }

    #[test]
    fn an_unclosed_marker_is_text_rather_than_a_panic() {
        let (prefix, struck, _) = split("half a ~~thought");
        assert_eq!(prefix, "half a ~~thought");
        assert!(struck.is_empty());
    }

    #[test]
    fn it_opens_with_what_it_was_given_and_does_not_move() {
        let tease = Tease::new("let's build something");
        assert_eq!(tease.shown(), "let's build something");
    }

    #[test]
    fn a_keystroke_puts_the_opener_back() {
        // Whatever it was mid-way through writing. A performance that insisted on finishing
        // would be arguing with somebody who has started typing.
        let mut tease = Tease::new("first");
        tease.shown = "half a lin".to_owned();
        tease.phase = Phase::Writing;
        tease.interrupt("second");
        assert_eq!(tease.shown(), "second");
        assert!(matches!(tease.phase, Phase::Resting(_)));
    }

    #[test]
    fn growing_stops_at_the_end_of_the_target() {
        let mut shown = "abc".to_owned();
        assert!(!grow(&mut shown, "abc"));
        assert_eq!(shown, "abc");
        assert!(grow(&mut shown, "abcd"));
        assert_eq!(shown, "abcd");
    }

    #[test]
    fn growing_counts_characters_rather_than_bytes() {
        // A line with an apostrophe or an accent in it would otherwise write half a character.
        let mut shown = String::new();
        let target = "café";
        for _ in 0..4 {
            assert!(grow(&mut shown, target));
        }
        assert_eq!(shown, "café");
        assert!(!grow(&mut shown, target));
    }
}
