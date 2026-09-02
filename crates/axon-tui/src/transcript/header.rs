//! The header: a reversed label, its arguments, and the handle that opens the block.
//!
//! Split out under THE RULE; the block renderer next door is what these are about.

use super::*;

mod tests {
    use super::*;

    fn rows(detail: Detail) -> Vec<String> {
        block(
            "write",
            r#"{"path":"/tmp/x","contents":"one\ntwo"}"#,
            Some(&axon_proto::ToolResult {
                output: "wrote /tmp/x".to_owned(),
                is_error: false,
            }),
            50,
            detail,
        )
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .collect()
    }

    #[test]
    fn a_folded_block_offers_to_open() {
        let shown = rows(Detail::Preview);
        assert!(
            shown
                .iter()
                .any(|l| l.contains(&format!("[ {} ]", crate::glyph::expand()))),
            "nothing says this can be opened: {shown:#?}"
        );
    }

    #[test]
    fn an_open_block_offers_to_fold() {
        let shown = rows(Detail::Full);
        assert!(
            shown
                .iter()
                .any(|l| l.contains(&format!("[ {} ]", crate::glyph::collapse()))),
            "and nothing says it can be closed again: {shown:#?}"
        );
    }

    #[test]
    fn the_handle_is_there_even_when_nothing_was_hidden() {
        // The "… N more lines" note only appears when the result was long enough to truncate.
        // A `write` reports one line, so without this its block had no handle at all — which is
        // exactly the block somebody wants to open, to read the file it wrote.
        let shown = rows(Detail::Preview);
        assert!(
            !shown.iter().any(|l| l.contains("more lines")),
            "the premise: nothing was truncated here: {shown:#?}"
        );
        assert!(
            shown
                .iter()
                .any(|l| l.contains(&format!("[ {} ]", crate::glyph::expand())))
        );
    }

    #[test]
    fn the_handle_rides_the_header_on_the_right() {
        // It was a row of its own at the foot of the box, which meant deciding whether to open
        // a block after reading to the end of what it was hiding.
        let shown = rows(Detail::Preview);
        let header = shown
            .iter()
            .position(|l| l.contains("write"))
            .expect("a header");
        assert_eq!(header, 1, "the first row after the padding");
        assert!(
            shown[header].contains(&format!("[ {} ]", crate::glyph::expand())),
            "the handle is set into the top edge: {shown:#?}"
        );
    }

    #[test]
    fn the_name_is_a_label_rather_than_a_word() {
        // Reversed out of the outcome colour, with a space either side so the run reads as a
        // tag on the block instead of as the first word of a sentence.
        let lines = block("write", "{}", None, 50, Detail::Preview);
        let name = lines[1]
            .spans
            .iter()
            .find(|s| s.content.contains("write"))
            .expect("the name");
        assert_eq!(name.content.as_ref(), "[ write ]");
        assert_eq!(
            name.style.bg,
            Some(colour::tool_title()),
            "the outcome behind"
        );
        assert_eq!(name.style.fg, Some(colour::tool_bg()), "the box in front");
    }

    #[test]
    fn the_label_carries_the_outcome() {
        let failed = &axon_proto::ToolResult {
            output: "no".to_owned(),
            is_error: true,
        };
        let lines = block("shell", "{}", Some(failed), 50, Detail::Preview);
        let name = lines[1]
            .spans
            .iter()
            .find(|s| s.content.contains("shell"))
            .expect("the name");
        assert_eq!(name.style.bg, Some(colour::tool_failed()));
    }

    #[test]
    fn a_narrow_screen_still_keeps_the_handle() {
        // The summary is clipped to make room rather than pushing the handle off the edge.
        for width in [20u16, 30, 44, 100] {
            let lines = block(
                "write",
                r#"{"path":"/a/very/long/path/that/keeps/going/on.rs"}"#,
                None,
                width,
                Detail::Preview,
            );
            let header: String = lines[1]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            assert_eq!(
                header.chars().count(),
                usize::from(width),
                "row must fill the width at {width}"
            );
            assert!(
                header.contains(&format!("[ {} ]", crate::glyph::expand())),
                "at {width}: {header:?}"
            );
        }
    }

    #[test]
    fn opening_a_block_does_not_repeat_what_the_header_said() {
        // A block used to list its arguments when opened, above a rule, above the output. For an
        // `edit` that is the same thing twice -- `old` and `new`, then a diff of `old` and `new`.
        // The summary beside the name is what the call was given, and once is enough.
        let shown = rows(Detail::Full);
        assert!(!shown.iter().any(|l| l == "one"), "{shown:#?}");
        assert!(
            !shown.iter().any(|l| l.starts_with('─')),
            "no rule: {shown:#?}"
        );
    }
}
