//! Reading `.gitignore`, without a regex engine.
//!
//! Not all of gitignore. What is here is the pattern forms people actually write in one —
//! `target/`, `*.log`, `/dist`, `node_modules`, `!keep.log` — matched by walking the string
//! once. The full grammar wants a glob compiler and a regex behind it, and that pair was eight
//! hundred kilobytes of binary to decide which filenames go in a popup.
//!
//! Where this and git disagree, it is on the exotic end — character classes, `**` in the middle
//! of a pattern — and it disagrees by *offering* a file rather than hiding one. A completion
//! list with one extra entry is a worse list; a completion list missing the file you wanted is
//! a broken feature.

use std::path::Path;

/// One line of a `.gitignore`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The pattern, with its markers stripped off.
    pattern: String,
    /// `!`: a match here un-ignores rather than ignores.
    negated: bool,
    /// A trailing `/`: only directories match.
    folders_only: bool,
    /// A leading or embedded `/`: matched against the whole path, not just the file name.
    anchored: bool,
}

impl Rule {
    /// Read one line, or `None` if it is blank or a comment.
    #[must_use]
    pub fn read(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (negated, line) = line
            .strip_prefix('!')
            .map_or((false, line), |after| (true, after));
        let (folders_only, line) = line
            .strip_suffix('/')
            .map_or((false, line), |before| (true, before));
        let anchored = line.contains('/');
        let pattern = line.strip_prefix('/').unwrap_or(line).to_owned();
        (!pattern.is_empty()).then_some(Self {
            pattern,
            negated,
            folders_only,
            anchored,
        })
    }

    /// Whether this rule has anything to say about `path`.
    ///
    /// An unanchored pattern is matched against every component, which is what makes
    /// `node_modules` in a root `.gitignore` hide one six directories down.
    #[must_use]
    pub fn covers(&self, path: &str, folder: bool) -> bool {
        if self.folders_only && !folder {
            return false;
        }
        if self.anchored {
            return matches(&self.pattern, path) || path.starts_with(&format!("{}/", self.pattern));
        }
        path.split('/').any(|part| matches(&self.pattern, part))
    }
}

/// Every rule in force, in the order they were read.
///
/// Order matters: gitignore is last-match-wins, so a `!` line after the rule it undoes is how
/// somebody keeps one file out of an ignored directory.
#[derive(Debug, Default)]
pub struct Ignores {
    rules: Vec<Rule>,
    /// Directories already read, so a walk that revisits one does not stack its rules twice.
    read: Vec<std::path::PathBuf>,
}

impl Ignores {
    /// The rules at the top of a tree.
    #[must_use]
    pub fn from(root: &Path) -> Self {
        let mut out = Self::default();
        out.read(root);
        out
    }

    /// Add whatever `dir/.gitignore` says, if it has not been read already.
    pub fn read(&mut self, dir: &Path) {
        if self.read.iter().any(|seen| seen == dir) {
            return;
        }
        self.read.push(dir.to_path_buf());
        let Ok(text) = std::fs::read_to_string(dir.join(".gitignore")) else {
            return;
        };
        self.rules.extend(text.lines().filter_map(Rule::read));
    }

    /// Whether `path`, relative to the root, is hidden by any of them.
    #[must_use]
    pub fn hides(&self, path: &str, folder: bool) -> bool {
        // Last match wins, so this walks backwards and stops at the first rule with an opinion.
        self.rules
            .iter()
            .rev()
            .find(|rule| rule.covers(path, folder))
            .is_some_and(|rule| !rule.negated)
    }
}

/// Whether `text` matches a glob of `*` and `?` and literals.
///
/// Iterative rather than recursive, with one backtrack point for the last `*` seen. That is the
/// whole algorithm: a pattern with twenty stars in it costs twenty steps, not two to the
/// twentieth, and a `.gitignore` from a stranger's repository cannot hang the completion popup.
#[must_use]
pub fn matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut at, mut here) = (0, 0);
    // Where to resume if the tail turns out not to match: just after the last `*`, one
    // character further into the text than last time.
    let (mut star, mut retry) = (None, 0);

    while here < text.len() {
        match pattern.get(at) {
            Some('*') => {
                star = Some(at);
                retry = here;
                at += 1;
            }
            Some('?') => {
                at += 1;
                here += 1;
            }
            Some(c) if *c == text[here] => {
                at += 1;
                here += 1;
            }
            _ => {
                let Some(back) = star else {
                    return false;
                };
                at = back + 1;
                retry += 1;
                here = retry;
            }
        }
    }
    // Trailing stars match the empty rest of the string; anything else left over does not.
    pattern[at..].iter().all(|c| *c == '*')
}

/// The patterns people write, and the ones they do not.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_matches_itself_and_nothing_else() {
        assert!(matches("target", "target"));
        assert!(!matches("target", "targets"));
        assert!(!matches("target", "arget"));
    }

    #[test]
    fn a_star_matches_any_run_including_none() {
        assert!(matches("*.log", "server.log"));
        assert!(matches("*.log", ".log"));
        assert!(!matches("*.log", "log.txt"));
        assert!(matches("build*", "build"));
        assert!(matches("*", "anything"));
    }

    #[test]
    fn a_question_mark_matches_exactly_one() {
        assert!(matches("v?.txt", "v1.txt"));
        assert!(!matches("v?.txt", "v10.txt"));
        assert!(!matches("v?.txt", "v.txt"));
    }

    #[test]
    fn several_stars_do_not_cost_exponential_time() {
        // The reason this backtracks once rather than recursing. A pattern like this against a
        // long string is the classic way to hang a naive matcher, and a `.gitignore` comes from
        // whatever repository you happen to have opened.
        let text = "a".repeat(64);
        assert!(!matches("*a*a*a*a*a*a*a*a*b", &text));
    }

    #[test]
    fn a_trailing_slash_means_directories_only() {
        let rule = Rule::read("target/").expect("a rule");
        assert!(rule.covers("target", true));
        assert!(!rule.covers("target", false), "a file called target stays");
    }

    #[test]
    fn an_unanchored_name_matches_at_any_depth() {
        // What makes one `node_modules` line at the top of a tree do its job.
        let rule = Rule::read("node_modules").expect("a rule");
        assert!(rule.covers("node_modules", true));
        assert!(rule.covers("web/app/node_modules", true));
    }

    #[test]
    fn a_leading_slash_pins_it_to_the_root() {
        let rule = Rule::read("/dist").expect("a rule");
        assert!(rule.covers("dist", true));
        assert!(!rule.covers("web/dist", true), "it is not the root's dist");
    }

    #[test]
    fn an_ignored_directory_takes_what_is_under_it() {
        let rule = Rule::read("target/").expect("a rule");
        assert!(rule.covers("target/debug/axon", true));
    }

    #[test]
    fn blank_lines_and_comments_are_not_rules() {
        assert_eq!(Rule::read(""), None);
        assert_eq!(Rule::read("   "), None);
        assert_eq!(Rule::read("# a comment"), None);
        assert_eq!(Rule::read("/"), None, "a bare slash says nothing");
    }

    #[test]
    fn the_last_rule_with_an_opinion_wins() {
        // How somebody keeps one file out of an ignored directory, and the reason the search
        // runs backwards.
        let ignores = Ignores {
            rules: ["*.log", "!keep.log"]
                .iter()
                .filter_map(|line| Rule::read(line))
                .collect(),
            ..Default::default()
        };
        assert!(ignores.hides("server.log", false));
        assert!(!ignores.hides("keep.log", false), "the negation is later");
    }

    #[test]
    fn nothing_is_hidden_by_an_empty_file() {
        let ignores = Ignores::default();
        assert!(!ignores.hides("anything/at/all", false));
    }

    #[test]
    fn the_same_directory_is_not_read_twice() {
        // The walk revisits directories as it descends, and rules stacked twice make a `!` line
        // lose to the copy of the rule it was written to undo.
        let dir = std::env::temp_dir().join(format!("axon-ignoring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(".gitignore"), "*.log\n!keep.log\n").expect("write");
        let mut ignores = Ignores::from(&dir);
        ignores.read(&dir);
        ignores.read(&dir);
        assert_eq!(ignores.rules.len(), 2, "the file was read more than once");
        assert!(!ignores.hides("keep.log", false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
