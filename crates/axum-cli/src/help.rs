//! What axum tells you about itself.
//!
//! The keys are written out because a binding is a `match` arm and a match cannot describe
//! itself. The commands are not: they come from the same list the completion popup offers, so
//! the two cannot say different things — which they already had, twice.

/// The keys `/help` lists.
///
/// Written out because a key binding is a `match` arm and a match cannot describe itself. The
/// commands are not: see [`text`].
const KEYS: &str = "\
**Keys**

- `enter` submit — `shift+enter` newline
- `esc` interrupt a running turn
- `tab` complete — `enter` runs the command — `↑/↓` move through the list
- `pgup`/`pgdn` scroll — `shift+↑/↓` by a line — `shift+home/end` to the ends
- `ctrl+o` show tool output in full, again to fold it back
- `ctrl+x` edit the prompt in `$EDITOR`
- `ctrl+c` clear the prompt, again to quit — `ctrl+d` quit
- `ctrl+a/e` line start/end — `ctrl+k/u` kill — `ctrl+y` yank
- `alt+←/→` word motion — `↑/↓` prompt history";

/// What `/help` prints.
///
/// The command list is built from the same one the completion popup offers, rather than
/// written out beside it. Two lists drift the moment either is edited, and this pair already
/// had: `/model` and `/rewind` were both offered by the popup and absent from the help of the
/// commit that added them.
pub fn text() -> String {
    let commands = axum_tui::complete::commands()
        .iter()
        .map(|c| format!("- `{}` {}", c.value, c.detail))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{KEYS}\n\n**Commands**\n\n{commands}\n\nType `@` to complete a path.")
}

#[cfg(test)]
mod help_tests {
    use super::*;

    #[test]
    fn every_command_the_popup_offers_is_in_the_help() {
        // The pair had already drifted: `/model` and `/rewind` were both offered and both
        // missing from the help of the commit that added them.
        let text = text();
        for candidate in axum_tui::complete::commands() {
            assert!(
                text.contains(&candidate.value),
                "{} is missing",
                candidate.value
            );
        }
    }

    #[test]
    fn the_help_says_how_to_scroll() {
        // Six bindings that existed since M0 and were documented nowhere.
        for key in ["pgup", "shift+↑/↓", "shift+home/end"] {
            assert!(text().contains(key), "{key}");
        }
    }

    #[test]
    fn the_help_is_markdown_the_transcript_can_render() {
        let text = text();
        assert!(text.contains("**Keys**") && text.contains("**Commands**"));
        assert!(text.ends_with("Type `@` to complete a path."));
    }
}
