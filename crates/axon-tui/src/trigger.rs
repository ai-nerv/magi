//! The sigils that mean something inside a prompt.
//!
//! `@src/main.rs` is a file, `$main/delta` is another instance, `/review` is a skill. Each is a
//! character that opens a completion and a token that ends at whitespace, and the only thing
//! that differs between them is where the candidates come from.
//!
//! A table rather than three branches in the resolver, because the third one was about to be
//! written as a copy of the second and the fourth would have been a copy of the third. What a
//! trigger *is* — its sigil, what it completes, whether it may appear mid-line — is data; how
//! its candidates are found is one function per trigger, supplied by the caller that has the
//! filesystem or the session list.
//!
//! # `/` means two things and they do not collide
//!
//! Pressed in normal mode, `/` starts a search of the transcript. Typed *inside prompt text* it
//! names a skill. Those are different contexts — one is a key with an empty buffer, the other is
//! a token in a sentence — in the same way `@` is a completion in the prompt and nothing at all
//! at the keyboard. Neither reading has to give way.

/// What a sigil opens.
///
/// The order is the order they are looked for, which matters only where one sigil could appear
/// inside another's token — a path with a `$` in it, say. Longest-standing first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// `:` — a command. Only at the start, and only on the command line.
    Command,
    /// `@` — a file in the project.
    File,
    /// `$` — another instance: a peer to ask, or a fork you own.
    Instance,
    /// `/` — a skill.
    Skill,
}

/// Every trigger, in the order a line is searched for one.
pub const EVERY: [Trigger; 4] = [
    Trigger::Command,
    Trigger::File,
    Trigger::Instance,
    Trigger::Skill,
];

impl Trigger {
    /// The character that opens it.
    #[must_use]
    pub fn sigil(self) -> char {
        match self {
            Self::Command => ':',
            Self::File => '@',
            Self::Instance => '$',
            Self::Skill => '/',
        }
    }

    /// Whether it only means anything at the very start of the line.
    ///
    /// A command is a whole line and the others are words in one: `:model` is the line, while
    /// `tell $gamma about @src/main.rs` has two triggers in the middle of a sentence.
    #[must_use]
    pub fn anchored(self) -> bool {
        matches!(self, Self::Command)
    }

    /// What to call this in a message to somebody.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::File => "file",
            Self::Instance => "instance",
            Self::Skill => "skill",
        }
    }

    /// Find this trigger's token in `before`, if the cursor is inside one.
    ///
    /// Answers where the sigil is and what has been typed after it. A token ends at whitespace,
    /// so `@src/ma` is a token and `@src/ma ` is a finished word nobody is still completing.
    #[must_use]
    pub fn found(self, before: &str) -> Option<Token> {
        let at = if self.anchored() {
            before.starts_with(self.sigil()).then_some(0)?
        } else {
            // A sigil only opens a trigger where a word does. Without this the `/` inside
            // `@src/main.rs` is a skill -- and it is *nearer the cursor* than the `@`, so it
            // wins, and every path completion turns into a skill completion halfway through
            // being typed. The last such position, so a second `@` on a line completes the
            // second and not the first.
            before
                .char_indices()
                .rev()
                .find(|(index, c)| {
                    *c == self.sigil()
                        && (*index == 0
                            || before[..*index]
                                .chars()
                                .next_back()
                                .is_some_and(char::is_whitespace))
                })
                .map(|(index, _)| index)?
        };
        let query = &before[at + self.sigil().len_utf8()..];
        if query.contains(char::is_whitespace) {
            return None;
        }
        Some(Token {
            trigger: self,
            at: before[..at].chars().count(),
            query: query.to_owned(),
        })
    }
}

/// A trigger's token, as it stands under the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Which sigil opened it.
    pub trigger: Trigger,
    /// Where the sigil is, in characters from the start of the line.
    pub at: usize,
    /// What has been typed after it.
    pub query: String,
}

impl Token {
    /// The token as written, sigil and all.
    #[must_use]
    pub fn written(&self) -> String {
        format!("{}{}", self.trigger.sigil(), self.query)
    }
}

/// The trigger under the cursor, if any, preferring the one nearest to it.
///
/// Nearest rather than first: `tell $gamma about @src/ma` has two, and the one being typed is
/// the one that ends at the cursor. Picking the first would complete a name the cursor left
/// several words ago.
#[must_use]
pub fn under(before: &str, wanted: &[Trigger]) -> Option<Token> {
    wanted
        .iter()
        .filter_map(|trigger| trigger.found(before))
        .max_by_key(|token| token.at)
}

/// Each sigil is its own, and a token ends where a word does.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sigil_is_different() {
        // Two triggers on one character is one of them never firing, and which one is decided
        // by the order of a match arm rather than by anything a reader would expect.
        let mut sigils: Vec<char> = EVERY.iter().map(|t| t.sigil()).collect();
        sigils.sort_unstable();
        let held = sigils.len();
        sigils.dedup();
        assert_eq!(sigils.len(), held, "two triggers share a sigil");
    }

    #[test]
    fn a_token_ends_at_whitespace() {
        // `@src/ma` is being typed; `@src/ma ` is a finished word and completing it would put
        // the popup back over a line somebody has moved on from.
        assert!(Trigger::File.found("@src/ma").is_some());
        assert!(Trigger::File.found("@src/ma ").is_none());
    }

    #[test]
    fn a_trigger_is_found_mid_sentence() {
        let token = Trigger::File
            .found("tell them about @src/ma")
            .expect("a token");
        assert_eq!(token.query, "src/ma");
        assert_eq!(token.at, 16);
    }

    #[test]
    fn the_last_one_wins_when_a_sigil_repeats() {
        // A second `@` on a line is the one being typed.
        let token = Trigger::File.found("@one @tw").expect("a token");
        assert_eq!(token.query, "tw");
    }

    #[test]
    fn a_command_only_counts_at_the_start() {
        assert!(Trigger::Command.found(":mod").is_some());
        assert!(
            Trigger::Command.found("a ratio of 3:4").is_none(),
            "a colon in a sentence is a colon"
        );
    }

    #[test]
    fn the_nearest_trigger_to_the_cursor_is_the_one_being_typed() {
        // The whole reason `under` sorts. Completing `$gamma` while the cursor is inside
        // `@src/ma` offers instance names for a path.
        let token = under("tell $gamma about @src/ma", &EVERY).expect("a token");
        assert_eq!(token.trigger, Trigger::File);
        assert_eq!(token.query, "src/ma");

        let token = under("about @src/main.rs tell $gam", &EVERY).expect("a token");
        assert_eq!(token.trigger, Trigger::Instance);
        assert_eq!(token.query, "gam");
    }

    #[test]
    fn nothing_is_triggered_by_ordinary_prose() {
        assert_eq!(under("just some words", &EVERY), None);
        assert_eq!(under("", &EVERY), None);
    }

    #[test]
    fn a_token_can_be_written_back_out() {
        let token = under("@src/ma", &EVERY).expect("a token");
        assert_eq!(token.written(), "@src/ma");
    }

    #[test]
    fn a_sigil_inside_a_word_is_not_a_trigger() {
        // The one that bit. A path has slashes in it, and the `/` in `@src/main.rs` is nearer
        // the cursor than the `@` -- so without a word boundary every path completion turns
        // into a skill completion halfway through being typed.
        let token = under("@src/main", &EVERY).expect("a token");
        assert_eq!(token.trigger, Trigger::File);
        assert_eq!(token.query, "src/main");
        assert_eq!(
            Trigger::Instance.found("costs $5 and 20$"),
            None,
            "a dollar in the middle of a word is a dollar"
        );
    }

    #[test]
    fn a_bare_sigil_is_a_token_with_an_empty_query() {
        // Typing `$` alone should offer every instance, the way `:` alone offers every command.
        let token = under("tell $", &EVERY).expect("a token");
        assert_eq!(token.trigger, Trigger::Instance);
        assert_eq!(token.query, "");
    }
}
