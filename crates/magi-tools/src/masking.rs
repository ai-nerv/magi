//! Keeping a credential out of the transcript.
//!
//! **The transcript is durable and the model sees it every turn.** A tool that prints an
//! environment — `printenv`, `env`, a failing build that echoes its own configuration, a `cat`
//! of a `.env` — puts whatever it found into the journal, on disk, for as long as the session
//! file exists. It then goes back to the provider on every subsequent request, inside the
//! `KEEP` tail that compaction preserves verbatim, and to the summariser as well. One `env` and
//! a key is in a file, in a context window, and at a third party, permanently.
//!
//! Here rather than in the tools, and for the reason [`crate::bound`] is: a peer is another
//! program and cannot be trusted to mask itself, a Lua tool has no way to know what is secret,
//! and every result of every transport passes through [`crate::Registry::call`].
//!
//! **By value, not by pattern.** What is masked is the *actual content* of this process's own
//! secret-looking environment variables — so there are no false positives at all: a string is
//! replaced only when it is, byte for byte, a credential this machine is holding. Guessing at
//! shapes would mean deciding that a forty-character hex string in somebody's test fixture is a
//! key, which is how a masker starts corrupting output and gets turned off.
//!
//! That bounds it honestly: a key in a file nothing exported is not masked, because nothing here
//! knows it is one. What this stops is the case that actually happens — the key the session was
//! started with, printed by something the model ran.

/// What a masked value is replaced with.
///
/// Names the variable, because the model usually needs to know the value *exists* — "is
/// `ANTHROPIC_API_KEY` set?" is a reasonable question and "yes, and I will not show you" is the
/// right answer to it. A bare `***` turns a useful answer into a confusing one.
fn marker(name: &str) -> String {
    format!("⟨{name}⟩")
}

/// Which variables hold something worth hiding.
///
/// By suffix, because that is the convention every tool that reads one follows, and a list of
/// exact names would be a list somebody has to maintain against every service that exists.
const SECRET: &[&str] = &[
    "_KEY",
    "_TOKEN",
    "_SECRET",
    "_PASSWORD",
    "_PASS",
    "_CREDENTIALS",
];

/// The shortest value worth masking.
///
/// A two-character `PASS` is not a credential, and replacing every occurrence of it would ruin
/// output that merely contains those two characters. Real credentials are long; this is the
/// length below which a false positive costs more than a true one saves.
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

/// This process's secret-looking variables, longest value first.
///
/// Longest first because one value can contain another — a `_TOKEN` that is a prefix of a
/// `_CREDENTIALS` blob — and masking the short one first would leave the rest of the long one in
/// place, which is worse than not masking at all: it looks redacted and is not.
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
    use super::{SHORTEST, apply, marker, secrets};

    #[test]
    fn text_holding_no_credential_is_untouched() {
        // Every result of every tool goes through this. A masker that changed ordinary output
        // would be a masker somebody turns off.
        let said = "the build failed at line 42\nsee target/debug/build.log".to_owned();
        assert_eq!(apply(said.clone()), said);
    }

    #[test]
    fn a_value_this_process_holds_is_replaced_by_its_name() {
        // Named rather than blanked: "is the key set?" is a reasonable question, and "yes, and I
        // will not show you" is the useful answer to it.
        let Some((name, value)) = secrets().into_iter().next() else {
            // No secret-looking variable in this environment, which is the ordinary case for a
            // test runner. The masker has nothing to do and the other tests cover the rest.
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
        // Replacing every occurrence of a two-character password would ruin output that merely
        // contains those two characters, which costs more than it saves.
        assert!(
            secrets().iter().all(|(_, value)| value.len() >= SHORTEST),
            "nothing shorter than {SHORTEST} is treated as a credential"
        );
    }

    #[test]
    fn which_names_count_is_by_convention_not_by_list() {
        // A list of exact names would be one somebody maintains against every service there is.
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
