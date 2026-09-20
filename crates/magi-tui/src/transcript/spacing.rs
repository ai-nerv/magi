//! One blank row before every entry, and never two.
//!
//! Split out under THE RULE; the join in `laid_out` next door is what these are about.

use super::*;

mod spacing {
    use super::tests::text_of;
    use super::*;
    use magi_proto::{MessageId, ToolCallId, ToolResult};

    fn user(text: &str) -> Entry {
        Entry::User {
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
                output: "out".into(),
                is_error: false,
                shown: None,
            }),
            thought_signature: None,
        }
    }

    fn shown(entries: &[Entry]) -> Vec<String> {
        text_of(&render(entries, 40, Detail::Preview))
            .iter()
            .map(|l| l.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn two_blocks_are_separated() {
        // A user message sat flush against whatever came after it, because a block pushed no gap
        // of its own and the tool call was the only entry that did.
        let rows = shown(&[user("one"), user("two")]);
        let second = rows
            .iter()
            .rposition(|l| l.starts_with('┌'))
            .expect("a second block");
        assert!(rows[second - 1].is_empty(), "{rows:#?}");
    }

    #[test]
    fn never_two_blank_rows_together() {
        // The other half. A tool call used to push its own gap as well, so two calls in a row
        // were parted by two rows and everything else by none.
        let rows = shown(&[user("one"), call("t1"), call("t2"), user("two")]);
        for pair in rows.windows(2) {
            assert!(
                !(pair[0].is_empty() && pair[1].is_empty()),
                "two blank rows: {rows:#?}"
            );
        }
    }

    #[test]
    fn every_block_after_the_first_has_a_gap_above_it() {
        let rows = shown(&[user("one"), call("t1"), user("two"), call("t2")]);
        for (at, line) in rows.iter().enumerate() {
            if at > 0 && line.starts_with('┌') {
                assert!(rows[at - 1].is_empty(), "no gap above row {at}: {rows:#?}");
            }
        }
    }

    #[test]
    fn nothing_is_wasted_above_the_first() {
        // A gap at the very top separates a block from nothing.
        let rows = shown(&[user("one")]);
        assert!(rows[0].starts_with('┌'), "{rows:#?}");
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
        let top = laid
            .lines
            .iter()
            .rposition(|l| text_of(std::slice::from_ref(l))[0].starts_with('┌'))
            .expect("the call");
        assert!(
            laid.owners[top].is_some(),
            "the block's own edge is unowned"
        );
    }
}
