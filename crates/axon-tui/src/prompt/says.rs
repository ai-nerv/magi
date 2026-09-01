//! What the box draws where there is no text: the placeholder, and the cursor on it.

use super::*;

/// The empty prompt says something, and says it as a placeholder.
#[cfg(test)]
mod placeholder_tests {
    use super::*;

    #[test]
    fn the_placeholder_is_dimmer_than_what_you_type() {
        // A placeholder in the text colour reads as something already in the box, and the first
        // thing anybody does is try to delete it.
        assert!(
            colour::palette().hint < colour::palette().text,
            "the hint is not dimmer: {} against {}",
            colour::palette().hint,
            colour::palette().text
        );
    }

    #[test]
    fn a_screen_too_narrow_for_the_line_says_something_shorter() {
        // Half a line reads as a rendering fault. The short hint stands in instead.
        let narrow = placeholder_spans(12, "a line far too long for twelve columns", None);
        let text: String = narrow.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.chars().count() <= 12, "{text:?}");
    }

    #[test]
    fn a_line_that_fits_is_drawn_whole() {
        let spans = placeholder_spans(40, "let's build something", None);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("let's build something"), "{text:?}");
    }

    #[test]
    fn nothing_is_struck_through_any_more() {
        // The correction is performed by `crate::tease` -- written, then taken back -- rather
        // than drawn with both halves on screen at once.
        let spans = placeholder_spans(60, "the scaffolding is temporary", None);
        assert!(
            spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
            "something is still drawn struck"
        );
    }
}

/// The cursor is a block in normal mode and a bar in insert mode.
#[cfg(test)]
mod cursor_shape_tests {
    use super::*;
    use crate::vim::Mode;

    /// Whether any cell of the drawn line is inverted.
    fn inverted(mode: Mode) -> bool {
        with_cursor("abc", 1, Style::default(), mode)
            .iter()
            .any(|span| span.style.add_modifier.contains(Modifier::REVERSED))
    }

    #[test]
    fn normal_mode_paints_the_character_under_the_cursor() {
        // A block sitting *on* a character, which is what the mode is: every key acts on the
        // thing under it.
        assert!(inverted(Mode::Normal));
    }

    #[test]
    fn insert_mode_leaves_it_to_the_terminal() {
        // Insert mode puts the cursor *between* two characters, and a whole cell painted over
        // one of them says the wrong thing about where the next letter will go.
        assert!(!inverted(Mode::Insert));
        assert!(!inverted(Mode::Command));
    }

    #[test]
    fn the_text_survives_either_way() {
        for mode in [Mode::Normal, Mode::Insert, Mode::Command] {
            let drawn: String = with_cursor("abc", 1, Style::default(), mode)
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(drawn, "abc", "{mode:?}");
        }
    }
}
