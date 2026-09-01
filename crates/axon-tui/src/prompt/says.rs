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
        let narrow = placeholder_spans(
            12,
            &crate::tease::Saying {
                text: "a line far too long for twelve columns",
                ..Default::default()
            },
        );
        let text: String = narrow.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.chars().count() <= 12, "{text:?}");
    }

    #[test]
    fn a_line_that_fits_is_drawn_whole() {
        let spans = placeholder_spans(
            40,
            &crate::tease::Saying {
                text: "let's build something",
                ..Default::default()
            },
        );
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("let's build something"), "{text:?}");
    }

    #[test]
    fn nothing_is_struck_through_any_more() {
        // The correction is performed by `crate::tease` -- written, then taken back -- rather
        // than drawn with both halves on screen at once.
        let spans = placeholder_spans(
            60,
            &crate::tease::Saying {
                text: "the scaffolding is temporary",
                ..Default::default()
            },
        );
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

/// The ghost cursor is on screen for every moment of a performance.
#[cfg(test)]
mod ghost_tests {
    use crate::prompt::placeholder_spans;
    use crate::tease::Saying;
    use ratatui::style::Modifier;

    /// The modifiers on the cell at `at`, once drawn.
    fn cell(saying: &Saying<'_>, at: usize) -> Modifier {
        placeholder_spans(60, saying)
            .into_iter()
            .flat_map(|span| {
                span.content
                    .chars()
                    .map(|_| span.style.add_modifier)
                    .collect::<Vec<_>>()
            })
            .nth(at)
            .unwrap_or_else(Modifier::empty)
    }

    #[test]
    fn a_moving_ghost_is_a_block() {
        let saying = Saying {
            text: "the roadmap is a list",
            caret: Some(4),
            block: true,
            ..Default::default()
        };
        assert!(cell(&saying, 4).contains(Modifier::REVERSED));
    }

    #[test]
    fn a_typing_ghost_is_still_on_screen() {
        // The bug this exists for. A bar belongs between two cells and a cell grid has no
        // between, so the insert ghost used to draw nothing -- and vanished for the whole of
        // the typing, which is most of the performance.
        let saying = Saying {
            text: "the roadmap is a list",
            caret: Some(4),
            block: false,
            ..Default::default()
        };
        let drawn = cell(&saying, 4);
        assert!(!drawn.is_empty(), "the ghost is invisible while it types");
        assert!(drawn.contains(Modifier::UNDERLINED));
        assert!(
            !drawn.contains(Modifier::REVERSED),
            "and it is not a block, which would say the wrong mode"
        );
    }

    #[test]
    fn it_shows_in_the_middle_of_the_line_as_well_as_the_end() {
        for at in [1usize, 7, 12] {
            for block in [true, false] {
                let saying = Saying {
                    text: "the roadmap is a list",
                    caret: Some(at),
                    block,
                    ..Default::default()
                };
                assert!(
                    !cell(&saying, at).is_empty(),
                    "nothing drawn at {at} with block={block}"
                );
            }
        }
    }

    #[test]
    fn a_marked_span_is_inverted_end_to_end() {
        let saying = Saying {
            text: "the roadmap is a list",
            caret: Some(4),
            block: true,
            marked: Some(4..11),
            ..Default::default()
        };
        for at in 4..11 {
            assert!(
                cell(&saying, at).contains(Modifier::REVERSED),
                "column {at} is not marked"
            );
        }
        assert!(!cell(&saying, 11).contains(Modifier::REVERSED), "and stops");
    }

    #[test]
    fn a_resting_box_shows_no_ghost_at_all() {
        // Only the real cursor on column zero. A second cursor on an untouched placeholder is a
        // second place to type, and there is only one.
        let saying = Saying {
            text: "the roadmap is a list",
            caret: None,
            ..Default::default()
        };
        for at in 1..20 {
            assert!(cell(&saying, at).is_empty(), "something drawn at {at}");
        }
    }
}
