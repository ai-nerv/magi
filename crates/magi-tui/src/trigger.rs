//! The sigils that mean something inside a prompt: `@src/main.rs` a file, `$main/delta` another
//! instance, `/review` a skill. Each is a character that opens a completion and a token that ends
//! at whitespace; what a trigger is, is data here, while how its candidates are found is one
//! function per trigger supplied by the caller. `/` in normal mode searches the transcript instead.

/// What a sigil opens, in the order they are looked for — which matters only where one sigil could
/// appear inside another's token.
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
    #[must_use]
    pub fn sigil(self) -> char {
        match self {
            Self::Command => ':',
            Self::File => '@',
            Self::Instance => '$',
            Self::Skill => '/',
        }
    }

    /// Whether it only means anything at the very start of the line. A command is a whole line;
    /// the others are words in one.
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

    /// Find this trigger's token in `before`, if the cursor is inside one. A token ends at
    /// whitespace, so `@src/ma` is one and `@src/ma ` is a finished word.
    #[must_use]
    pub fn found(self, before: &str) -> Option<Token> {
        let at = if self.anchored() {
            before.starts_with(self.sigil()).then_some(0)?
        } else {
            // A sigil only opens a trigger where a word does, or the `/` inside `@src/main.rs` is
            // a skill and is nearer the cursor than the `@`. The last such position.
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
    pub trigger: Trigger,
    /// Where the sigil is, in characters from the start of the line.
    pub at: usize,
    pub query: String,
}

impl Token {
    /// The token as written, sigil and all.
    #[must_use]
    pub fn written(&self) -> String {
        format!("{}{}", self.trigger.sigil(), self.query)
    }
}

/// The trigger under the cursor, if any, preferring the nearest — the one that ends at the cursor.
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
        // Two triggers on one character is one of them never firing.
        let mut sigils: Vec<char> = EVERY.iter().map(|t| t.sigil()).collect();
        sigils.sort_unstable();
        let held = sigils.len();
        sigils.dedup();
        assert_eq!(sigils.len(), held, "two triggers share a sigil");
    }

    #[test]
    fn a_token_ends_at_whitespace() {
        // `@src/ma` is being typed; `@src/ma ` is a finished word.
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
        // The whole reason `under` sorts: otherwise `$gamma` offers instance names for a path.
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
        // A path has slashes in it, and the `/` in `@src/main.rs` is nearer the cursor than the
        // `@`, so without a word boundary every path completion turns into a skill completion.
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

/// Every finished token of this trigger in a line. Unlike [`Trigger::found`], which stops at the
/// first space, this asks what a finished sentence names: `tell $gamma to stop` names `$gamma`.
#[must_use]
pub fn named(line: &str, trigger: Trigger) -> Vec<String> {
    line.split_whitespace()
        .filter_map(|word| word.strip_prefix(trigger.sigil()))
        .filter(|rest| !rest.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// A finished sentence names what it mentions.
#[cfg(test)]
mod naming_tests {
    use super::*;

    #[test]
    fn a_finished_sentence_names_what_it_mentions() {
        // `found` stops at the first space; this asks what an already-sent line is talking about.
        assert_eq!(
            named("tell $main/delta to stop", Trigger::Instance),
            vec!["main/delta"]
        );
        assert!(
            Trigger::Instance
                .found("tell $main/delta to stop")
                .is_none(),
            "which the completion form does not answer"
        );
    }

    #[test]
    fn several_are_all_named() {
        assert_eq!(
            named("ask $gamma and $delta", Trigger::Instance),
            vec!["gamma", "delta"]
        );
    }

    #[test]
    fn a_bare_sigil_names_nobody() {
        assert!(named("costs $ and more", Trigger::Instance).is_empty());
    }

    #[test]
    fn a_sigil_inside_a_word_is_not_a_name() {
        assert!(named("that costs 20$", Trigger::Instance).is_empty());
    }

    #[test]
    fn a_path_is_not_an_instance() {
        assert!(named("look at @src/main.rs", Trigger::Instance).is_empty());
        assert_eq!(
            named("look at @src/main.rs", Trigger::File),
            vec!["src/main.rs"]
        );
    }
}
