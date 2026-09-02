//! The two ends of a header: the reversed name, and the chip that opens the block.
//!
//! Split out under THE RULE; the block renderer next door is what these are about.

use super::*;

mod tests {
    use super::*;

    /// The span carrying the fold handle, which is not the last: `pad` fills to the width
    /// after it.
    fn handle_span(line: &Line<'static>) -> Span<'static> {
        line.spans
            .iter()
            .find(|s| {
                s.content.contains(crate::glyph::expand())
                    || s.content.contains(crate::glyph::collapse())
            })
            .expect("the handle")
            .clone()
    }

    fn header_of(err: bool) -> Line<'static> {
        let result = axon_proto::ToolResult {
            output: "out".to_owned(),
            is_error: err,
        };
        block(
            "shell",
            r#"{"command":"ls"}"#,
            Some(&result),
            50,
            Detail::Preview,
        )[1]
        .clone()
    }

    #[test]
    fn the_handle_is_reversed_like_the_name() {
        // It was a lone bright glyph at the far end of an empty row, which reads as debris
        // rather than as the other end of the same header.
        let line = header_of(false);
        // Neither is at an end of the span list: `pad` puts the block's own padding around
        // everything the header drew.
        let name = line
            .spans
            .iter()
            .find(|s| s.content.contains("shell"))
            .expect("the name")
            .clone();
        let handle = handle_span(&line);
        assert_eq!(handle.style.bg, name.style.bg, "the same chip");
        assert_eq!(handle.style.fg, name.style.fg);
    }

    #[test]
    fn a_failed_call_colours_both_ends_by_its_outcome() {
        let ok = header_of(false);
        let failed = header_of(true);
        assert_ne!(
            handle_span(&ok).style.fg,
            handle_span(&failed).style.fg,
            "the handle states the outcome the way the name does"
        );
    }

    #[test]
    fn a_block_is_separated_from_the_one_before_it() {
        // Painted with the block's own background this row joined the previous block's bottom
        // padding into one two-row band, and three calls in a row read as a single wall.
        let result = axon_proto::ToolResult {
            output: "out".to_owned(),
            is_error: false,
        };
        let lines = block("shell", "{}", Some(&result), 50, Detail::Preview);
        assert_eq!(
            lines.first().expect("a leading row").spans[0].style.bg,
            None,
            "the gap between blocks carries no background"
        );
        assert_eq!(
            lines.last().expect("a trailing row").spans[0].style.bg,
            None,
            "the bottom edge is outside the fill too"
        );
    }
}
