//! Keeping a credential out of the transcript. The journal is durable and the model sees it every
//! turn, so one `printenv` puts a key in a file, in a context window and at a third party. Done
//! here rather than in the tools because every result of every transport passes through
//! [`crate::Registry::call`]. Masked by value, not by pattern, so there are no false positives: a
//! key in a file nothing exported is not masked, because nothing here knows it is one.

use magi_proto::tooling::{Role, Shown, Span};

/// What a masked value is replaced with — the name, because "is the key set?" is a fair question.
fn marker(name: &str) -> String {
    format!("⟨{name}⟩")
}

/// Which variables hold something worth hiding, by suffix rather than by a list of exact names.
const SECRET: &[&str] = &[
    "_KEY",
    "_TOKEN",
    "_SECRET",
    "_PASSWORD",
    "_PASS",
    "_CREDENTIALS",
];

/// The shortest value worth masking. A two-character `PASS` is not a credential, and replacing
/// every occurrence of it would ruin output that merely contains those two characters.
const SHORTEST: usize = 12;

/// Replace any credential this process is holding with a marker naming it.
#[must_use]
pub fn apply(content: String) -> String {
    let mut out = content;
    for (name, value) in secrets() {
        if out.contains(&value) {
            out = out.replace(&value, &marker(&name));
        }
    }
    out
}

/// What a tool painted, masked as its text is. A line holding a credential is kept whole and plain,
/// so a key split across highlighted pieces cannot survive between them.
#[must_use]
pub fn painted(shown: Option<Shown>) -> Option<Shown> {
    let Some(Shown::Painted { lines }) = shown else {
        return shown;
    };
    let lines = lines
        .into_iter()
        .map(|line| {
            let whole: String = line.iter().map(|span| span.text.as_str()).collect();
            let masked = apply(whole.clone());
            if masked == whole {
                return line;
            }
            let back = line.first().and_then(|span| span.back);
            vec![Span {
                back,
                ..Span::new(Role::Text, masked)
            }]
        })
        .collect();
    Some(Shown::Painted { lines })
}

/// This process's secret-looking variables, longest value first: one value can contain another, and
/// masking the short one first would leave the rest of the long one in place, which looks redacted.
pub(crate) fn secrets() -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = std::env::vars()
        .filter(|(name, value)| {
            value.len() >= SHORTEST
                && SECRET
                    .iter()
                    .any(|suffix| name.to_uppercase().ends_with(suffix))
        })
        .collect();
    found.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
    found
}

#[cfg(test)]
mod tests {
    use super::{SHORTEST, apply, marker, painted, secrets};
    use magi_proto::tooling::{Role, Shown, Span};

    #[test]
    fn a_painted_line_holding_a_credential_is_masked_whole() {
        let Some((name, value)) = secrets().into_iter().next() else {
            return;
        };
        let half = value
            .char_indices()
            .nth(value.chars().count() / 2)
            .map_or(0, |(at, _)| at);
        let (head, tail) = value.split_at(half);
        let shown = Some(Shown::Painted {
            lines: vec![vec![
                Span::new(Role::String, head),
                Span::new(Role::Text, tail),
            ]],
        });
        let Some(Shown::Painted { lines }) = painted(shown) else {
            panic!("still a painting");
        };
        let text: String = lines[0].iter().map(|span| span.text.as_str()).collect();
        assert!(!text.contains(&value), "the value is gone: {text}");
        assert!(text.contains(&marker(&name)), "and named: {text}");
    }

    #[test]
    fn a_painting_holding_no_credential_keeps_its_colours() {
        let shown = Some(Shown::Painted {
            lines: vec![vec![Span::new(Role::Keyword, "fn")]],
        });
        assert_eq!(painted(shown.clone()), shown);
    }

    #[test]
    fn text_holding_no_credential_is_untouched() {
        let said = "the build failed at line 42\nsee target/debug/build.log".to_owned();
        assert_eq!(apply(said.clone()), said);
    }

    #[test]
    fn a_value_this_process_holds_is_replaced_by_its_name() {
        let Some((name, value)) = secrets().into_iter().next() else {
            // No secret-looking variable in this environment, the ordinary case for a test runner.
            return;
        };
        let printed = format!("{name}={value}\nPATH=/usr/bin");
        let masked = apply(printed);
        assert!(!masked.contains(&value), "the value is gone: {masked}");
        assert!(masked.contains(&marker(&name)), "and named: {masked}");
        assert!(masked.contains("PATH=/usr/bin"), "the rest survives");
    }

    #[test]
    fn a_short_value_is_not_worth_masking() {
        assert!(
            secrets().iter().all(|(_, value)| value.len() >= SHORTEST),
            "nothing shorter than {SHORTEST} is treated as a credential"
        );
    }

    #[test]
    fn which_names_count_is_by_convention_not_by_list() {
        for name in [
            "ANTHROPIC_API_KEY",
            "GITHUB_TOKEN",
            "aws_secret",
            "DB_PASSWORD",
        ] {
            assert!(
                super::SECRET
                    .iter()
                    .any(|suffix| name.to_uppercase().ends_with(suffix)),
                "{name} is not recognised"
            );
        }
        for name in ["PATH", "HOME", "MAGI_MODEL", "KEYBOARD"] {
            assert!(
                !super::SECRET
                    .iter()
                    .any(|suffix| name.to_uppercase().ends_with(suffix)),
                "{name} is not a credential"
            );
        }
    }
}
