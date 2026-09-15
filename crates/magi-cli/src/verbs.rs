//! `magi verbs` — what this program answers, on each of its doors. The command line half is read
//! off clap rather than listed by hand, so the surface cannot have a second definition.

use clap::CommandFactory;
use magi_ipc::Wire;
use magi_ipc::family::Reply;

/// The revision of the *registrar* surface — what a third party writes against, separate from the
/// family revision a [`Reply`] carries. See EXTENDING.md.
const SURFACE: u16 = 1;

/// How the caller asked to be answered. [`As::Bare`] is what a person gets when they name neither
/// encoding, and is the only one that may be prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum As {
    Bare,
    Json,
    Cbor,
}

impl As {
    /// What the command line asked for. `--cbor` wins: only a caller that will not read the
    /// answer asks for bytes.
    #[must_use]
    pub fn asked(json: bool, cbor: bool) -> Self {
        match (json, cbor) {
            (_, true) => Self::Cbor,
            (true, _) => Self::Json,
            _ => Self::Bare,
        }
    }

    /// Whether an encoding was named, and so whether the answer owes the reply shape.
    #[must_use]
    pub fn framed(self) -> bool {
        self != Self::Bare
    }
}

/// Write one reply to stdout in the encoding asked for; [`As::Bare`] means JSON. JSON gets a
/// trailing newline, CBOR is bytes and gets nothing.
pub fn say(reply: &Reply, how: As) {
    use std::io::Write;
    let cbor = how == As::Cbor;
    let wire = if cbor { Wire::Cbor } else { Wire::Json };
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&reply.encode(wire));
    if !cbor {
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
}

/// Print the surface, in the family's reply shape.
pub fn print(how: As) {
    let listed: Vec<serde_json::Value> = super::Cli::command()
        .get_subcommands()
        .map(|sub| {
            serde_json::json!({
                "verb": sub.get_name(),
                "about": sub.get_about().map(|a| a.to_string()).unwrap_or_default(),
                "door": "cli",
            })
        })
        .collect();

    say(&Reply::rows(listed).on_surface(SURFACE), how);
}

#[cfg(test)]
mod tests {
    use super::As;

    #[test]
    fn naming_no_encoding_is_what_a_person_gets() {
        assert!(!As::asked(false, false).framed());
        assert!(As::asked(true, false).framed());
        assert!(As::asked(false, true).framed());
    }

    #[test]
    fn bytes_win_over_text_when_both_are_named() {
        assert_eq!(As::asked(true, true), As::Cbor);
    }
}
