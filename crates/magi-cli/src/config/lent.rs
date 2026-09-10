//! Client libraries, asked from the siblings that implement them rather than vendored: a vendored
//! copy goes stale and silently drops tools the surface it talks to still declares.

/// Ask each of these for its client library — the programs filling this session's roles, from
/// [`super::roles`]. Absent, unrunnable, or lending nothing is not an error.
#[must_use]
pub fn borrowed(siblings: &[String]) -> Vec<(String, String)> {
    siblings
        .iter()
        .filter_map(|name| lends(name).map(|source| (name.clone(), source)))
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

/// The Lua in what a sibling answered, or nothing when it answered a refusal: a refusal is the
/// family's JSON reply shape, and no Lua chunk begins with `{`.
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
        // What casper answers; loading it as Lua would be a syntax error at config time.
        assert!(library(r#"{"ok":false,"family":1,"n":0,"result":[],"error":"none"}"#).is_none());
        assert!(library("").is_none());
        assert!(library("   \n ").is_none());
    }

    #[test]
    fn lua_is_taken_verbatim() {
        let source = "-- a client\nlocal M = {}\nreturn M\n";
        assert_eq!(library(source).as_deref(), Some(source));
    }

    #[test]
    fn a_sibling_that_is_not_installed_lends_nothing() {
        assert!(lends("magi-no-such-sibling-anywhere").is_none());
    }
}
