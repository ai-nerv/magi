//! How a turn that stopped early, and a rewind, are drawn.
//!
//! Split out under THE RULE; the transcript next door is what these are about.

#[cfg(test)]
mod stop_tests {
    use crate::transcript::tests::text_of;
    use crate::transcript::*;
    use magi_proto::MessageId;

    fn stopped(reason: StopReason, error: Option<&str>) -> Entry {
        Entry::Assistant {
            id: MessageId::new("s"),
            text: "half an answer".into(),
            thinking: String::new(),
            stop_reason: Some(reason),
            error: error.map(ToOwned::to_owned),
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        }
    }

    #[test]
    fn an_interrupt_reads_as_one() {
        // "Operation aborted" is a machine's word for a key the reader just pressed.
        let lines = text_of(&entry_lines(
            &stopped(StopReason::Aborted, None),
            40,
            Detail::Preview,
        ));
        assert!(lines.iter().any(|l| l.contains("Interrupted")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("aborted")), "{lines:?}");
    }

    #[test]
    fn an_interrupt_is_not_coloured_as_a_failure() {
        // Red claims something went wrong; the reader asked for this.
        let rendered = entry_lines(&stopped(StopReason::Aborted, None), 40, Detail::Preview);
        let note = rendered
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("Interrupted"))
            .expect("the note");
        assert_ne!(note.style.fg, Some(colour::error()));
    }

    #[test]
    fn what_arrived_before_the_interrupt_is_kept() {
        let lines = text_of(&entry_lines(
            &stopped(StopReason::Aborted, None),
            40,
            Detail::Preview,
        ));
        assert!(
            lines.iter().any(|l| l.contains("half an answer")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_real_failure_is_still_red() {
        let rendered = entry_lines(
            &stopped(StopReason::Error, Some("no route")),
            40,
            Detail::Preview,
        );
        let note = rendered
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("no route"))
            .expect("the note");
        assert_eq!(note.style.fg, Some(colour::error()));
    }

    #[test]
    fn an_abort_that_came_with_a_reason_says_the_reason() {
        let lines = text_of(&entry_lines(
            &stopped(StopReason::Aborted, Some("the daemon went away")),
            40,
            Detail::Preview,
        ));
        assert!(
            lines.iter().any(|l| l.contains("daemon went away")),
            "{lines:?}"
        );
    }
}

#[cfg(test)]
mod branch_tests {
    use crate::transcript::tests::text_of;
    use crate::transcript::*;
    use magi_proto::MessageId;

    fn rewound(keeps: usize) -> Entry {
        Entry::Branch {
            id: MessageId::new("b"),
            keeps,
        }
    }

    #[test]
    fn a_rewind_says_what_it_did_rather_than_where_it_landed() {
        // "rewound to message 0" is a journal index. Nobody has one of those in mind.
        let lines = text_of(&entry_lines(&rewound(0), 80, Detail::Preview));
        let joined = lines.join(" ");
        assert!(joined.contains("nothing above is sent"), "{joined}");
        assert!(!joined.contains("message 0"), "{joined}");
    }

    #[test]
    fn a_partial_rewind_says_how_much_it_kept() {
        let lines = text_of(&entry_lines(&rewound(4), 80, Detail::Preview));
        assert!(lines.join(" ").contains("first 4"), "{lines:?}");
    }

    #[test]
    fn the_rule_still_spans_the_width() {
        let lines = entry_lines(&rewound(2), 60, Detail::Preview);
        let widest = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| crate::wrap::columns(&s.content))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        assert_eq!(widest, 60);
    }
}
