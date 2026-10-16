//! One blank row before every entry, and never two.
//!
//! Split out under THE RULE; the join in `laid_out` next door is what these are about.

use super::*;

mod spacing {
    use super::*;
    use magi_proto::{MessageId, ToolCallId, ToolResult};

    fn user(text: &str) -> Entry {
        Entry::User {
            images: Vec::new(),
            id: MessageId::new(text),
            text: text.into(),
            aside: String::new(),
        }
    }

    fn call(id: &str) -> Entry {
        Entry::Tool {
            id: ToolCallId::new(id),
            name: "shell".into(),
            args: r#"{"command":"ls"}"#.into(),
            result: Some(ToolResult {
                images: Vec::new(),
                output: "out".into(),
                is_error: false,
                shown: None,
            }),
            thought_signature: None,
        }
    }

    /// Each row, and whether it is a gap between blocks.
    fn gaps(entries: &[Entry]) -> Vec<(Line<'static>, bool)> {
        render(entries, 40, Detail::Preview)
            .into_iter()
            .map(|line| {
                let gap = blank_row(Some(&line));
                (line, gap)
            })
            .collect()
    }

    #[test]
    fn two_blocks_are_separated() {
        let rows = gaps(&[user("one"), user("two")]);
        let second = rows
            .iter()
            .rposition(|(line, _)| is_header(line))
            .expect("a second block");
        assert!(rows[second - 1].1, "{rows:#?}");
    }

    #[test]
    fn never_two_blank_rows_together() {
        let rows = gaps(&[user("one"), call("t1"), call("t2"), user("two")]);
        for pair in rows.windows(2) {
            assert!(!(pair[0].1 && pair[1].1), "two blank rows: {rows:#?}");
        }
    }

    #[test]
    fn every_block_after_the_first_has_a_gap_above_it() {
        let rows = gaps(&[user("one"), call("t1"), user("two"), call("t2")]);
        for (at, (line, _)) in rows.iter().enumerate() {
            if at > 0 && is_header(line) {
                assert!(rows[at - 1].1, "no gap above row {at}: {rows:#?}");
            }
        }
    }

    #[test]
    fn a_closing_row_of_fill_is_not_a_gap() {
        let rows = gaps(&[user("one")]);
        let (last, gap) = rows.last().expect("a closing row");
        assert!(!gap, "{last:#?}");
    }

    #[test]
    fn nothing_is_wasted_above_the_first() {
        let rows = gaps(&[user("one")]);
        assert!(is_header(&rows[0].0), "{rows:#?}");
    }

    #[test]
    fn the_line_a_click_lands_on_still_belongs_to_its_block() {
        // The separator is a row like any other on screen, so it has to be counted — an owner
        // list a row short would map every click after it to the block above.
        let laid = laid_out(
            &[user("one"), call("t1")],
            40,
            Detail::Preview,
            &BTreeSet::new(),
        );
        assert_eq!(laid.lines.len(), laid.owners.len(), "a row with no owner");
        let top = laid.lines.iter().rposition(is_header).expect("the call");
        assert!(
            laid.owners[top].is_some(),
            "the block's own header is unowned"
        );
    }
}
