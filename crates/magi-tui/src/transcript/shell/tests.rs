//! Which part of a command gets which colour, and how much of it a block shows.
//!
//! Split out under THE RULE; the highlighter these check is next door.

use super::*;

/// Each piece of `line` beside the colour it was drawn in, whitespace left out.
fn painted(line: &str) -> Vec<(String, Option<Color>)> {
    highlight(line, Style::default())
        .into_iter()
        .filter(|span| !span.content.trim().is_empty())
        .map(|span| (span.content.into_owned(), span.style.fg))
        .collect()
}

fn ink_of(line: &str, piece: &str) -> Option<Color> {
    painted(line)
        .into_iter()
        .find(|(text, _)| text == piece)
        .and_then(|(_, ink)| ink)
}

#[test]
fn every_command_in_a_pipeline_is_a_command() {
    let line = "git log --oneline | head -5 && echo done";
    assert_eq!(ink_of(line, "git"), Some(colour::code_command()));
    assert_eq!(ink_of(line, "head"), Some(colour::code_command()));
    assert_eq!(ink_of(line, "echo"), Some(colour::code_command()));
    assert_eq!(ink_of(line, "--oneline"), Some(colour::code_flag()));
    assert_eq!(ink_of(line, "|"), Some(colour::code_operator()));
    assert_eq!(ink_of(line, "&&"), Some(colour::code_operator()));
    assert_eq!(ink_of(line, "done"), Some(colour::code_argument()));
}

#[test]
fn a_program_that_takes_a_subcommand_has_it_marked() {
    let line = "sudo cargo build --release -p magi-cli";
    assert_eq!(ink_of(line, "sudo"), Some(colour::code_command()));
    assert_eq!(
        ink_of(line, "cargo"),
        Some(colour::code_command()),
        "what sudo runs"
    );
    assert_eq!(ink_of(line, "build"), Some(colour::code_subcommand()));
    assert_eq!(ink_of(line, "--release"), Some(colour::code_flag()));
    assert_eq!(
        ink_of(line, "magi-cli"),
        Some(colour::code_argument()),
        "only the first"
    );
    // A program with no subcommands keeps its first argument an argument.
    assert_eq!(ink_of("echo hello", "hello"), Some(colour::code_argument()));
    // A wrapper's own flags and numbers come before the command it runs.
    let line = "timeout 5 git -C x status";
    assert_eq!(ink_of(line, "5"), Some(colour::code_number()));
    assert_eq!(ink_of(line, "git"), Some(colour::code_command()));
}

#[test]
fn a_flag_with_a_value_is_split_at_the_equals() {
    let pieces = painted("ls --color=always");
    assert!(
        pieces.contains(&("--color".to_owned(), Some(colour::code_flag()))),
        "{pieces:?}"
    );
    assert!(
        pieces.contains(&("=".to_owned(), Some(colour::code_operator()))),
        "{pieces:?}"
    );
    assert!(
        pieces.contains(&("always".to_owned(), Some(colour::code_argument()))),
        "{pieces:?}"
    );
}

#[test]
fn paths_and_numbers_are_told_from_plain_arguments() {
    let line = "cp ./src ~/x a.rs /etc/hosts 42 1.5 word";
    for path in ["./src", "~/x", "a.rs", "/etc/hosts"] {
        assert_eq!(ink_of(line, path), Some(colour::code_path()), "{path}");
    }
    assert_eq!(ink_of(line, "42"), Some(colour::code_number()));
    assert_eq!(ink_of(line, "1.5"), Some(colour::code_number()));
    assert_eq!(ink_of(line, "word"), Some(colour::code_argument()));
}

#[test]
fn the_shells_own_words_are_keywords() {
    let line = "for f in *.rs; do echo $f; done";
    assert_eq!(ink_of(line, "for"), Some(colour::code_keyword()));
    assert_eq!(ink_of(line, "in"), Some(colour::code_keyword()));
    assert_eq!(ink_of(line, "*.rs"), Some(colour::code_path()));
    assert_eq!(ink_of(line, "do"), Some(colour::code_keyword()));
    assert_eq!(ink_of(line, "echo"), Some(colour::code_command()));
    assert_eq!(ink_of(line, "done"), Some(colour::code_keyword()));
}

#[test]
fn strings_variables_and_comments_have_their_own_colours() {
    let line = r#"FOO=1 grep "a b" $HOME ${X} $? 'c' # why"#;
    assert_eq!(ink_of(line, "FOO"), Some(colour::code_variable()));
    assert_eq!(
        ink_of(line, "grep"),
        Some(colour::code_command()),
        "still the command"
    );
    assert_eq!(ink_of(line, r#""a b""#), Some(colour::code_string()));
    assert_eq!(ink_of(line, "'c'"), Some(colour::code_string()));
    assert_eq!(ink_of(line, "$HOME"), Some(colour::code_variable()));
    assert_eq!(ink_of(line, "${X}"), Some(colour::code_variable()));
    assert_eq!(ink_of(line, "$?"), Some(colour::code_variable()));
    assert_eq!(ink_of(line, "# why"), Some(colour::code_comment()));
}

#[test]
fn a_redirect_is_followed_by_a_path_and_a_heredoc_by_its_delimiter() {
    assert_eq!(ink_of("cat a > out", "out"), Some(colour::code_path()));
    assert_eq!(ink_of("make 2>&1", "1"), Some(colour::code_number()));
    assert_eq!(ink_of("cat <<EOF", "EOF"), Some(colour::code_string()));
    assert_eq!(ink_of("x | wc -l", "wc"), Some(colour::code_command()));
}

#[test]
fn nothing_is_lost_or_added() {
    for line in [
        "git log --oneline | head -5 && echo done",
        r#"echo "it's \"quoted\"" >> f; ls -la $(pwd)/x"#,
        "sudo -u me FOO=1 cargo run -- --flag=a\\ b 2>&1 | tee `date`.log",
        "for f in *.rs; do echo $f; done",
        "unterminated 'quote",
        "trailing \\",
        "",
    ] {
        let drawn: String = highlight(line, Style::default())
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(drawn, line);
    }
}

#[test]
fn only_a_shell_call_is_highlighted() {
    assert_eq!(
        command_of("shell", r#"{"command":"ls -la"}"#).as_deref(),
        Some("ls -la")
    );
    assert_eq!(command_of("read", r#"{"command":"ls"}"#), None);
    assert_eq!(command_of("shell", r#"{"path":"x"}"#), None);
}

#[test]
fn opened_the_command_keeps_its_own_lines_and_folded_it_is_one() {
    let args = r#"{"command":"cat > a.py <<'EOF'\nprint(1)\nprint(2)\nEOF"}"#;
    let rows = |detail| {
        asked("shell", args, detail, Style::default(), 60, 50, 4)
            .expect("a shell call")
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    let open = rows(Detail::Full);
    assert_eq!(open.len(), 4, "{open:#?}");
    assert!(open[1].contains("print(1)"), "{open:#?}");
    assert_eq!(rows(Detail::Preview).len(), 1);
}

#[test]
fn a_cut_ends_in_an_ellipsis_and_fits() {
    let spans = highlight("git log --oneline --graph --all", Style::default());
    let kept = clipped(spans, 12);
    let text: String = kept.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(crate::wrap::columns(&text), 12, "{text:?}");
    assert!(text.ends_with('…'), "{text:?}");
}
