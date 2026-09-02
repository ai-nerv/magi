//! The two ends of a header: the reversed name, and the chip that opens the block.
//!
//! Split out under THE RULE; the block renderer next door is what these are about.

use super::*;

mod tests {
    use super::*;
    /// The span carrying the tool's name, which is what states the outcome.
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
        )[0]
        .clone()
    }

    #[test]
    fn the_handle_belongs_to_the_frame_rather_than_the_name() {
        // The name says what this block *is* and carries a colour for it. The handle is the same
        // affordance on every block that has one, so it is drawn like the line it sits in — and
        // a reader is not asked to read meaning into a shape that never varies.
        let line = header_of(false);
        let name = line
            .spans
            .iter()
            .find(|s| s.content.contains("shell"))
            .expect("the name")
            .clone();
        let handle = handle_span(&line);
        assert_eq!(handle.style.fg, Some(colour::border()));
        assert_ne!(handle.style.fg, name.style.fg, "the handle apes the name");
    }

    #[test]
    fn the_brackets_are_the_frames_too() {
        // Only the text inside them is the block's own colour. Painted with it, the punctuation
        // read as the signal and every block wore a solid tag.
        let line = header_of(false);
        for span in line.spans.iter().filter(|s| s.content.contains('[')) {
            assert_eq!(span.style.fg, Some(colour::border()), "{:?}", span.content);
        }
    }

    #[test]
    fn only_the_name_states_the_outcome() {
        let ok = header_of(false);
        let failed = header_of(true);
        assert_ne!(
            named(&ok).style.fg,
            named(&failed).style.fg,
            "the name should state the outcome"
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
