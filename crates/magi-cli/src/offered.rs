//! What the program filling the `tools` role says it offers.
//!
//! Asked of that program directly, the way a run is asked of balthasar: the session's own registry
//! holds Lua tools and builtins as well, and casper's float is about casper.

/// Read its `tools` reply. Empty where it could not be run or would not answer in the shape.
#[must_use]
pub fn fetch(program: &str) -> Vec<magi_tui::tooling::Tool> {
    let Ok(done) = std::process::Command::new(program)
        .args(["tools", "--json"])
        .output()
    else {
        return Vec::new();
    };
    let Ok(reply) = serde_json::from_slice::<serde_json::Value>(&done.stdout) else {
        return Vec::new();
    };
    reply["result"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|row| {
            let said = |key: &str| row[key].as_str().unwrap_or_default().to_owned();
            magi_tui::tooling::Tool {
                name: said("name"),
                group: said("group"),
                needs: row["needs"].as_str().map(ToOwned::to_owned),
                deferred: row["deferred"].as_bool().unwrap_or(false),
                about: said("description"),
            }
        })
        .filter(|tool| !tool.name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_is_not_there_offers_nothing_rather_than_panicking() {
        assert!(fetch("definitely-not-a-program-here").is_empty());
    }

    #[test]
    fn a_reply_that_is_not_the_shape_is_no_tools_rather_than_a_guess() {
        assert!(fetch("true").is_empty());
    }
}
