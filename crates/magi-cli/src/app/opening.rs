//! The empty prompt: what it opens with, and when it starts performing.
//!
//! Split out under THE RULE; `App::settle_prompt` next door is what these are about.

use super::*;

mod settling {
    use super::{App, opener};

    #[test]
    fn it_opens_with_an_opener_not_a_joke() {
        // What somebody reads when they sit down. A punchline in this position is a punchline
        // in the way.
        let app = App::new();
        assert!(
            magi_tui::glyph::openers()
                .iter()
                .any(|o| o == app.tease.shown()),
            "{:?} is not one of the openers",
            app.tease.shown()
        );
    }

    #[test]
    fn an_opener_carries_no_marker() {
        // The openers are written, not performed. A `~~` in one would be typed out literally.
        for line in magi_tui::glyph::openers() {
            assert!(
                !line.contains("~~"),
                "{line:?} is a performance, not an opener"
            );
        }
    }

    #[test]
    fn typing_puts_the_opener_back() {
        let mut app = App::new();
        app.tease.restart("mid-performance");
        app.editor.insert_str("a question");
        app.settle_prompt();
        assert!(
            magi_tui::glyph::openers()
                .iter()
                .any(|o| o == app.tease.shown()),
            "{:?}",
            app.tease.shown()
        );
    }

    #[test]
    fn a_submitted_prompt_leaves_an_opener_behind() {
        // The other way a prompt empties, and the one that happens every turn.
        let mut app = App::new();
        app.editor.insert_str("a question");
        app.settle_prompt();
        app.editor.submit();
        app.settle_prompt();
        assert!(
            magi_tui::glyph::openers()
                .iter()
                .any(|o| o == app.tease.shown()),
            "{:?}",
            app.tease.shown()
        );
    }

    #[test]
    fn it_does_not_start_before_its_time() {
        // Thirty seconds by default. A box that started rewriting itself the moment you looked
        // away from it would be a box you could never read.
        let mut app = App::new();
        let opened = app.tease.shown().to_owned();
        for _ in 0..200 {
            app.settle_prompt();
        }
        assert_eq!(app.tease.shown(), opened);
    }

    #[test]
    fn an_opener_is_always_available_even_with_none_configured() {
        // `openers()` falls back to the built-in line, so this can never be empty.
        assert!(!opener().is_empty());
    }
}
