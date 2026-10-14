//! What reaches the terminal is text and never an instruction to it. A C0 control other than tab
//! and newline, DEL, a C1 control and a bidirectional override are each shown as a visible
//! stand-in one column wide, wherever the text came from.

use std::borrow::Cow;

/// The visible stand-in for `c`, or nothing when it prints as itself.
#[must_use]
pub fn stand_in(c: char) -> Option<char> {
    match c {
        '\t' | '\n' => None,
        '\0'..='\x1f' => char::from_u32(0x2400 + u32::from(c)),
        '\x7f' => Some('\u{2421}'),
        '\u{80}'..='\u{9f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}'
        | '\u{200e}'
        | '\u{200f}'
        | '\u{61c}' => Some('\u{fffd}'),
        _ => None,
    }
}

/// `text` with every character that would instruct the terminal replaced by its stand-in.
#[must_use]
pub fn visible(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| stand_in(c).is_some()) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.chars().map(|c| stand_in(c).unwrap_or(c)).collect())
}

/// The same, over every cell of a finished frame, which is the last place text passes before it is
/// written to the terminal.
pub fn scrub(buffer: &mut ratatui::buffer::Buffer) {
    for cell in &mut buffer.content {
        if cell.symbol().chars().any(|c| stand_in(c).is_some()) {
            let shown = visible(cell.symbol()).into_owned();
            cell.set_symbol(&shown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_escape_a_bell_a_c1_and_an_override_are_shown_not_obeyed() {
        let shown = visible("a\x1b]52;c;ZXZpbA==\x07b\u{9b}2Jc\u{202e}d\re\x7f");
        assert_eq!(
            shown,
            "a\u{241b}]52;c;ZXZpbA==\u{2407}b\u{fffd}2Jc\u{fffd}d\u{240d}e\u{2421}"
        );
        assert!(!shown.chars().any(|c| c.is_control()));
    }

    #[test]
    fn tabs_newlines_and_ordinary_text_pass_unchanged() {
        let said = "fn main() {\n\tprintln!(\"héllo — 世界\");\n}";
        assert!(matches!(visible(said), Cow::Borrowed(_)));
    }

    #[test]
    fn each_stand_in_is_one_column_wide() {
        use unicode_width::UnicodeWidthChar;
        for c in ['\x1b', '\x07', '\r', '\x7f', '\u{9b}', '\u{202e}'] {
            let shown = stand_in(c).expect("a stand-in");
            assert_eq!(shown.width(), Some(1), "{c:?} as {shown:?}");
        }
    }
}
