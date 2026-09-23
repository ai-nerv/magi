use super::*;
use magi_model::Message;

fn talking(rounds: usize, each: usize) -> Context {
    Context {
        system: Some("you are magi".to_owned()),
        messages: (0..rounds)
            .map(|n| Message::user("word ".repeat(each) + &n.to_string()))
            .collect(),
        tools: Vec::new(),
    }
}

/// Nothing goes out above what the policy leaves, however it was chosen.
mod mem_dispatch_never_exceeds_policy {
    use super::*;

    #[test]
    fn a_request_inside_the_limit_is_admitted() {
        let small = talking(2, 10);
        let admission = admit(&small, Some(128_000), 8_000);
        assert!(admission.admitted(), "{admission:?}");
    }

    #[test]
    fn a_request_over_the_limit_is_not() {
        // Half of a 32k window is 16k, and this is far past it.
        let huge = talking(400, 200);
        let admission = admit(&huge, Some(32_000), 4_000);
        assert!(!admission.admitted(), "{admission:?}");
        assert!(admission.over_by() > 0);
    }

    #[test]
    fn the_limit_is_half_the_window_while_the_reply_leaves_room_for_it() {
        for (window, reply, limit) in [
            (32_000_u64, 4_000_u64, 16_000_u64),
            (128_000, 8_000, 64_000),
            (200_000, 32_000, 100_000),
            (1_000_000, 64_000, 500_000),
        ] {
            let room = Room::of(Some(window), reply, 0).expect("a usable window");
            assert_eq!(room.limit, limit, "{window} window");
            assert_eq!(room.capacity, window, "the real window is kept");
        }
    }

    #[test]
    fn the_reservation_binds_where_it_is_the_smaller_of_the_two() {
        let room = Room::of(Some(32_000), 24_000, 0).expect("usable");
        assert_eq!(room.ceiling, 16_000);
        assert_eq!(room.limit, 8_000);
    }

    #[test]
    fn what_is_counted_is_the_whole_request_not_only_the_conversation() {
        // Instructions and declarations occupy the window as much as the messages do; counting
        // the conversation alone is how a request passes a check and overflows anyway.
        let bare = Context {
            system: None,
            messages: vec![Message::user("hello")],
            tools: Vec::new(),
        };
        let dressed = Context {
            system: Some("a long system instruction ".repeat(50)),
            tools: vec![magi_model::Tool {
                name: "read".to_owned(),
                description: "read a file ".repeat(20),
                parameters: serde_json::json!({ "type": "object" }),
            }],
            ..bare.clone()
        };
        assert!(
            counted(&dressed) > counted(&bare) + 100,
            "{} vs {}",
            counted(&dressed),
            counted(&bare)
        );
    }
}

/// A window nobody reported is said so, not treated as room enough.
mod mem_unknown_window_is_explicit {
    use super::*;

    #[test]
    fn an_unknown_window_blocks_rather_than_admitting_everything() {
        let admission = admit(&talking(1, 5), None, 4_000);
        assert_eq!(admission, Admission::Blocked(Blocked::UnknownWindow));
        assert!(!admission.admitted());
    }

    #[test]
    fn a_zero_window_is_unknown_rather_than_a_window_of_nothing() {
        assert_eq!(
            Room::of(Some(0), 1_000, 0),
            Err(Blocked::UnknownWindow),
            "a card that reported nothing arrives here as zero"
        );
    }
}

/// A model whose reservation or margin leaves no room says so once, rather than per request.
mod mem_unsatisfiable_mandatory_input {
    use super::*;

    #[test]
    fn a_reply_that_fills_the_window_blocks() {
        assert_eq!(
            Room::of(Some(8_000), 8_000, 0),
            Err(Blocked::ReplyFillsWindow {
                window: 8_000,
                reply: 8_000
            })
        );
        assert!(matches!(
            admit(&talking(1, 1), Some(8_000), 9_000),
            Admission::Blocked(Blocked::ReplyFillsWindow { .. })
        ));
    }

    #[test]
    fn a_margin_wider_than_the_room_blocks() {
        assert_eq!(
            Room::of(Some(16_000), 1_000, 9_000),
            Err(Blocked::MarginFillsRoom {
                room: 8_000,
                margin: 9_000
            })
        );
    }

    #[test]
    fn a_blocked_model_is_blocked_whatever_the_request_holds() {
        for rounds in [1_usize, 100] {
            assert!(matches!(
                admit(&talking(rounds, 5), None, 1_000),
                Admission::Blocked(_)
            ));
        }
    }
}

/// The reply and the margin have to fit beside the input, not only under the limit.
mod whole_window {
    use super::*;

    #[test]
    fn the_reply_and_margin_are_counted_against_the_window() {
        let room = Room::of(Some(32_000), 8_000, 2_000).expect("usable");
        assert_eq!(room.ceiling, 16_000, "the share is what bound the room");
        assert_eq!(room.limit, 14_000, "and the margin comes off it");
        assert!(room.admits(14_000));
        assert!(!room.admits(14_001));
    }

    #[test]
    fn a_count_that_would_overflow_the_sum_is_refused_rather_than_wrapping() {
        let room = Room::of(Some(32_000), 1_000, 0).expect("usable");
        assert!(!room.admits(u64::MAX));
    }
}

/// What the boundary does with a request it has refused, in each mode.
mod mem_rejects_false_fits {
    use super::*;

    #[test]
    fn a_refusal_names_what_was_counted_and_what_was_left() {
        let room = Room::of(Some(40_000), 8_000, 0).expect("usable");
        let said = refusal(Admission::Over {
            counted: 34_864,
            room,
        });
        assert!(said.contains("34864"), "{said}");
        assert!(said.contains("20000"), "{said}");
        assert!(said.contains(COUNTING), "the counting is named: {said}");
        assert!(said.contains("Nothing was sent"), "{said}");
    }

    #[test]
    fn each_way_of_being_blocked_says_which_it_was() {
        let said = |blocked| refusal(Admission::Blocked(blocked));
        assert!(said(Blocked::UnknownWindow).contains("never reported"));
        assert!(
            said(Blocked::ReplyFillsWindow {
                window: 8_000,
                reply: 8_000
            })
            .contains("no room for a question")
        );
        assert!(
            said(Blocked::MarginFillsRoom {
                room: 8_000,
                margin: 9_000
            })
            .contains("safety margin")
        );
    }

    #[test]
    fn an_admitted_request_has_nothing_to_say() {
        let room = Room::of(Some(40_000), 8_000, 0).expect("usable");
        assert!(
            refusal(Admission::Admitted { counted: 10, room }).is_empty(),
            "a request that fits is not explained"
        );
    }

    #[test]
    fn the_boundary_refuses_what_it_measures_over() {
        // It measured without refusing while the memory layer still planned against what the
        // window had left after the reply. It plans to `L` now, so a count over `L` is a request
        // that should never have been built, and the boundary says so.
        assert_eq!(
            [ENFORCING],
            [true],
            "the boundary enforces now that the memory layer plans to `L`"
        );
        let over = admit(&talking(400, 200), Some(32_000), 4_000);
        assert!(!over.admitted(), "measured as not fitting");
        assert!(over.over_by() > 0, "and by how much is known");
        assert!(
            !refusal(over).is_empty(),
            "and what is refused says why in the caller's words"
        );
    }
}

/// What is shown about a request is what was actually done with it.
mod mem_context_diagnostics_match_dispatch {
    use super::*;
    use magi_proto::HarnessEvent;

    fn fields(event: &HarnessEvent) -> (u64, u64, u64, String, String) {
        match event {
            HarnessEvent::RequestAdmitted {
                capacity,
                limit,
                counted,
                counting,
                outcome,
                ..
            } => (
                *capacity,
                *limit,
                *counted,
                counting.clone(),
                outcome.clone(),
            ),
            other => panic!("not an admission: {other:?}"),
        }
    }

    #[test]
    fn an_admitted_request_reports_the_count_the_boundary_used() {
        let small = talking(2, 10);
        let said = admit(&small, Some(128_000), 8_000);
        let (capacity, limit, count, counting, outcome) = fields(&reported(said, "admitted"));
        assert_eq!(capacity, 128_000, "the real window, not the cap");
        assert_eq!(limit, 64_000);
        assert_eq!(
            count,
            counted(&small),
            "the same number, not a second count"
        );
        assert_eq!(counting, COUNTING);
        assert_eq!(outcome, "admitted");
    }

    #[test]
    fn a_refused_request_carries_why_and_an_admitted_one_does_not() {
        let over = admit(&talking(400, 200), Some(32_000), 4_000);
        let HarnessEvent::RequestAdmitted { why, .. } = reported(over, "refused") else {
            panic!("not an admission");
        };
        assert!(why.contains(COUNTING), "{why}");
        let fits = admit(&talking(1, 5), Some(128_000), 8_000);
        let HarnessEvent::RequestAdmitted { why, .. } = reported(fits, "admitted") else {
            panic!("not an admission");
        };
        assert!(why.is_empty(), "{why}");
    }

    #[test]
    fn a_blocked_model_reports_no_room_rather_than_a_room_of_zero_it_measured() {
        let blocked = admit(&talking(1, 5), None, 4_000);
        let (capacity, limit, count, _, outcome) = fields(&reported(blocked, "blocked"));
        assert_eq!((capacity, limit, count), (0, 0, 0));
        assert_eq!(outcome, "blocked");
    }

    #[test]
    fn the_confidence_of_the_count_is_stated_rather_than_assumed() {
        // A cap certified against an estimate is not a certificate, so what it was is said.
        let said = reported(admit(&talking(1, 5), Some(128_000), 8_000), "admitted");
        let (.., counting, _) = fields(&said);
        assert!(
            counting == "estimated" || counting == "verified",
            "{counting}"
        );
    }
}

/// Nothing a diagnostic carries is the conversation itself.
mod mem_diagnostics_do_not_leak_sources {
    use super::*;

    const SENTINEL: &str = "SENTINEL-LOCAL-ONLY";

    #[test]
    fn what_is_reported_is_numbers_and_never_the_text_counted() {
        let holding = Context {
            system: Some(format!("the deploy key is {SENTINEL}")),
            messages: vec![magi_model::Message::user(format!("and again: {SENTINEL}"))],
            tools: vec![magi_model::Tool {
                name: SENTINEL.to_owned(),
                description: SENTINEL.to_owned(),
                parameters: serde_json::json!({ "secret": SENTINEL }),
            }],
        };
        for outcome in ["admitted", "refused", "blocked"] {
            let said = reported(admit(&holding, Some(32_000), 4_000), outcome);
            let shown = serde_json::to_string(&said).expect("json");
            assert!(!shown.contains(SENTINEL), "{outcome}: {shown}");
        }
    }

    #[test]
    fn a_refusal_names_sizes_and_not_content() {
        let over = admit(&talking(400, 200), Some(32_000), 4_000);
        let said = refusal(over);
        assert!(!said.contains("word"), "the messages themselves: {said}");
        assert!(said.contains(COUNTING), "{said}");
    }
}
