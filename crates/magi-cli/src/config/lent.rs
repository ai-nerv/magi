//! Client libraries, from the programs that implement them.
//!
//! **A copy of somebody else's client is a copy that goes stale.** magi shipped balthasar's — 631
//! lines of it — and one that had fallen behind silently removed every memory tool from every
//! session on a machine, which is how the practice was found to be a bad one. The library and the
//! surface it talks to are one thing, and they ship together or they disagree.
//!
//! So every program in the family answers `client`, and magi asks rather than vendors. A sibling
//! that is not installed lends nothing, which is right: the tools its library would declare could
//! not have worked anyway.
//!
//! `oslo` and `hexe` are still vendored, because neither answers `client` yet. They are outside
//! this repository and outside the family contract; when they implement it, their copies go the
//! same way this one did.

/// Every sibling that might lend magi a library.
///
/// Named rather than discovered, for the same reason the coordinator's list is: what magi expects
/// to find is a fact about magi, and a list built by looking would answer "what is installed".
const SIBLINGS: &[&str] = &["casper", "melchior", "balthasar"];

/// Ask each sibling for its client library.
///
/// Quiet about every kind of no. A sibling that is absent, will not run, or has no library to
/// lend — casper says so in as many words — is not an error, and a session that refused to start
/// over one would be worse than one that carries on without it.
#[must_use]
pub fn borrowed() -> Vec<(String, String)> {
    SIBLINGS
        .iter()
        .filter_map(|name| lends(name).map(|source| ((*name).to_owned(), source)))
        .collect()
}

/// What one sibling lends, if it lends anything.
fn lends(program: &str) -> Option<String> {
    let out = std::process::Command::new(program)
        .arg("client")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let source = String::from_utf8(out.stdout).ok()?;
    library(&source)
}

/// The Lua in what a sibling answered, or nothing when it answered a refusal.
///
/// A library is Lua and a refusal is the family's reply shape, so the first character tells them
/// apart: JSON's top level is `{`, and no Lua chunk begins with one. casper answers the refusal —
/// its surface is reached by spawning it with a call, not from a VM — and that is an answer,
/// where silence would not be.
fn library(source: &str) -> Option<String> {
    let text = source.trim_start();
    if text.is_empty() || text.starts_with('{') {
        return None;
    }
    Some(source.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_not_mistaken_for_a_library() {
        // What casper answers. Loading it as Lua would be a syntax error at config time, from a
        // sibling that behaved correctly.
        assert!(library(r#"{"ok":false,"family":1,"n":0,"result":[],"error":"none"}"#).is_none());
        assert!(library("").is_none());
        assert!(library("   \n ").is_none());
    }

    #[test]
    fn lua_is_taken_verbatim() {
        // Including the leading comment, which is the library's own documentation: a consumer
        // that redirects this to a file wants the whole of it.
        let source = "-- a client\nlocal M = {}\nreturn M\n";
        assert_eq!(library(source).as_deref(), Some(source));
    }

    #[test]
    fn a_sibling_that_is_not_installed_lends_nothing() {
        assert!(lends("magi-no-such-sibling-anywhere").is_none());
    }
}
