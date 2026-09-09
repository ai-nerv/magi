//! What magi tells you about itself.

/// The keys `:help` lists, written out because a `match` arm cannot describe itself.
const KEYS: &str = "\
**Keys**

- `enter` submit — `shift+enter` newline
- `esc` leave insert mode — again in normal mode to interrupt a running turn
- `i a I A o O` insert — `esc` back — `u` undo
- `h j k l` `w b e` `0 ^ $` `f t F T` move — `gg G` ends
- `d c y` take a motion: `dw` `d$` `ct,` `yb` — doubled for the line: `dd cc yy`
- `x X D p P J ~ r` edit — `:` command line, which holds your prompt until it closes

**Triggers** — `@file` completes a path, `$instance` another magi, `/skill` a skill (not built yet)
- `j k` scroll the transcript while the prompt is one line — `gg G` its ends
- `tab` complete — `enter` runs the command — `↑/↓` move through the list
- `pgup`/`pgdn` scroll — `shift+↑/↓` by a line — `shift+home/end` to the ends
- `ctrl+o` show tool output in full, again to fold it back
- click a block's `▸` to open just that one — it wraps what a preview cut
- drag to select; it is copied when you let go
- `ctrl+x` edit the prompt in `$EDITOR`
- `ctrl+c` clear the prompt — `:q` to leave, `:qa` to take everything with it
- `ctrl+a/e` line start/end — `ctrl+k/u` kill — `ctrl+y` yank
- `alt+←/→` word motion — `↑/↓` prompt history
- `alt+,`/`alt+.` move to the previous or next agent — the `< >` at the bottom left
  says there is somewhere to go. A peer's screen is read-only: what you type goes
  nowhere until you come back to your own.";

/// What `:help` prints. The command list is built from the same one the completion popup offers,
/// rather than written out beside it; two lists drift the moment either is edited.
pub fn text() -> String {
    let commands = magi_tui::complete::commands()
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
        let text = text();
        for candidate in magi_tui::complete::commands() {
            assert!(
                text.contains(&candidate.value),
                "{} is missing",
                candidate.value
            );
        }
    }

    #[test]
    fn the_help_says_how_to_scroll() {
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
