//! The two ends of a header: the reversed name, and the chip that opens the block.
//!
//! Split out under THE RULE; the block renderer next door is what these are about.

use super::*;

mod tests {
    use super::*;
    use ratatui::style::Modifier;
    /// The span carrying the tool's name.
    fn named(line: &Line<'static>) -> Span<'static> {
        line.spans
            .iter()
            .find(|s| s.content.contains("shell"))
            .expect("the name")
            .clone()
    }

    /// The span carrying the fold handle, which is not the last: `pad` fills to the width
    /// after it.
    fn handle_span(line: &Line<'static>) -> Span<'static> {
        line.spans
            .iter()
            .find(|s| s.content.contains(crate::glyph::collapse()))
            .expect("the handle")
            .clone()
    }

    fn header_of(err: bool) -> Line<'static> {
        let result = magi_proto::ToolResult {
            images: Vec::new(),
            output: "out".to_owned(),
            is_error: err,
            shown: None,
        };
        block(
            "shell",
            r#"{"command":"ls"}"#,
            Some(&result),
            50,
            Detail::Preview,
        )[0]
        .clone()
    }

    #[test]
    fn the_handle_is_drawn_like_the_rule_it_sits_in() {
        let line = header_of(false);
        let handle = handle_span(&line);
        assert_eq!(handle.style.fg, Some(colour::tool_bg()));
        assert!(handle.style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn the_brackets_are_the_rules_too() {
        let line = header_of(false);
        for span in line.spans.iter().filter(|s| s.content.contains('[')) {
            assert_eq!(span.style.fg, Some(colour::tool_bg()), "{:?}", span.content);
            assert!(
                span.style.add_modifier.contains(Modifier::REVERSED),
                "{:?}",
                span.content
            );
        }
    }

    #[test]
    fn the_name_states_the_outcome() {
        let ok = header_of(false);
        let failed = header_of(true);
        assert_eq!(named(&ok).style.fg, Some(colour::tool_ok()));
        assert_eq!(named(&failed).style.fg, Some(colour::tool_failed()));
        assert_eq!(named(&ok).style.bg, Some(colour::tool_bg()), "on the fill");
    }

    #[test]
    fn a_block_is_separated_from_the_one_before_it() {
        // Painted with the block's own background this row joined the previous block's bottom
        // padding into one two-row band, and three calls in a row read as a single wall.
        let result = magi_proto::ToolResult {
            images: Vec::new(),
            output: "out".to_owned(),
            is_error: false,
            shown: None,
        };
        let lines = block("shell", "{}", Some(&result), 50, Detail::Preview);
        assert_eq!(
            lines.first().expect("a leading row").spans[0].style.bg,
            None,
            "the header's first column is the screen's"
        );
        assert_eq!(
            lines.last().expect("a trailing row").spans[0].style.bg,
            None,
            "and so is the closing row's"
        );
    }
}
