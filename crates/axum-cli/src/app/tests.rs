//! What the reduction is supposed to do.
//!
//! Split out under THE RULE: the state machine and its tests both grow, and a file holding
//! both crossed 800 lines while each half was still the right size.

mod reduction {
    use super::super::*;
    use axum_proto::{StopReason, ToolResult};

    fn app_with(events: Vec<HarnessEvent>) -> App {
        let mut app = App::new();
        for event in events {
            app.apply(event);
        }
        app
    }

    #[test]
    fn deltas_accumulate_onto_the_started_message() {
        let app = app_with(vec![
            HarnessEvent::AssistantStarted {
                cursor: Cursor(1),
                id: MessageId::new("a1"),
            },
            HarnessEvent::AssistantDelta {
                cursor: Cursor(2),
                id: MessageId::new("a1"),
                text: "hel".into(),
                thinking: String::new(),
            },
            HarnessEvent::AssistantDelta {
                cursor: Cursor(3),
                id: MessageId::new("a1"),
                text: "lo".into(),
                thinking: String::new(),
            },
        ]);
        match &app.entries()[0] {
            Entry::Assistant { text, .. } => assert_eq!(text, "hello"),
            other => panic!("expected an assistant entry, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_result_lands_on_its_call() {
        let app = app_with(vec![
            HarnessEvent::ToolCallStarted {
                cursor: Cursor(1),
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: "{}".into(),
            },
            HarnessEvent::ToolCallEnded {
                cursor: Cursor(2),
                id: ToolCallId::new("t1"),
                result: ToolResult {
                    output: "ok".into(),
                    is_error: false,
                },
            },
        ]);
        match &app.entries()[0] {
            Entry::Tool { result, .. } => {
                assert_eq!(result.as_ref().map(|r| r.output.as_str()), Some("ok"));
            }
            other => panic!("expected a tool entry, got {other:?}"),
        }
    }

    #[test]
    fn the_cursor_tracks_the_highest_event_seen() {
        let app = app_with(vec![
            HarnessEvent::StatusChanged {
                cursor: Cursor(5),
                status: AgentStatus::Idle,
            },
            HarnessEvent::StatusChanged {
                cursor: Cursor(9),
                status: AgentStatus::Idle,
            },
        ]);
        assert_eq!(app.cursor(), Cursor(9));
    }

    #[test]
    fn a_reordered_event_cannot_rewind_the_cursor() {
        let app = app_with(vec![
            HarnessEvent::StatusChanged {
                cursor: Cursor(9),
                status: AgentStatus::Idle,
            },
            HarnessEvent::StatusChanged {
                cursor: Cursor(2),
                status: AgentStatus::Idle,
            },
        ]);
        assert_eq!(app.cursor(), Cursor(9));
    }

    #[test]
    fn the_last_entry_never_settles_because_it_may_still_stream() {
        let app = app_with(vec![HarnessEvent::UserMessage {
            cursor: Cursor(1),
            id: MessageId::new("m1"),
            text: "hi".into(),
        }]);
        assert_eq!(app.settled(), Flush::Nothing);
    }

    #[test]
    fn earlier_entries_settle_once_a_later_one_arrives() {
        let mut app = app_with(vec![
            HarnessEvent::UserMessage {
                cursor: Cursor(1),
                id: MessageId::new("m1"),
                text: "hi".into(),
            },
            HarnessEvent::AssistantStarted {
                cursor: Cursor(2),
                id: MessageId::new("a1"),
            },
        ]);
        assert_eq!(app.settled(), Flush::Upto(1));
        app.mark_flushed(1);
        assert_eq!(app.settled(), Flush::Nothing);
        assert_eq!(app.live().len(), 1);
    }

    #[test]
    fn a_reattach_snapshot_does_not_reprint_what_is_already_on_screen() {
        let mut app = App::new();
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor(9),
            session: axum_proto::SessionId::new("s"),
            entries: vec![
                Entry::User {
                    id: MessageId::new("m1"),
                    text: "hi".into(),
                },
                Entry::Assistant {
                    id: MessageId::new("a1"),
                    text: "partial".into(),
                    thinking: String::new(),
                    stop_reason: None,
                    error: None,
                    signatures: axum_proto::Signatures::default(),
                    usage: axum_proto::Usage::default(),
                },
            ],
            status: AgentStatus::Idle,
            model: None,
            choices: Vec::new(),
            thinking: String::new(),
        });
        assert_eq!(
            app.live().len(),
            1,
            "only the in-flight entry is redrawn; the rest is already in scrollback"
        );
        assert_eq!(app.settled(), Flush::Nothing, "nothing new to write");
    }

    #[test]
    fn a_cold_snapshot_still_renders_everything() {
        let mut app = App::new();
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor::ZERO,
            session: axum_proto::SessionId::new("s"),
            entries: vec![Entry::User {
                id: MessageId::new("m9"),
                text: "restored".into(),
            }],
            status: AgentStatus::Idle,
            model: None,
            choices: Vec::new(),
            thinking: String::new(),
        });
        assert_eq!(app.live().len(), 1, "a cold snapshot is entirely unflushed");
    }

    #[test]
    fn clearing_the_view_empties_the_transcript_and_the_flush_mark() {
        let mut app = app_with(vec![
            HarnessEvent::UserMessage {
                cursor: Cursor(1),
                id: MessageId::new("m1"),
                text: "hi".into(),
            },
            HarnessEvent::AssistantStarted {
                cursor: Cursor(2),
                id: MessageId::new("a1"),
            },
        ]);
        app.mark_flushed(1);
        app.clear_view();
        assert!(app.entries().is_empty());
        assert!(app.live().is_empty());
        assert_eq!(
            app.cursor(),
            Cursor(2),
            "clearing the view keeps the position"
        );
    }

    #[test]
    fn the_popup_follows_the_prompt() {
        let mut app = App::new();
        let none = |_: &str| Vec::new();
        app.editor.insert_str("/qu");
        app.refresh_completion(&none);
        assert!(app.completion.is_some());
        app.editor.clear();
        app.editor.insert_str("plain text");
        app.refresh_completion(&none);
        assert!(app.completion.is_none());
    }

    #[test]
    fn busy_tracks_status() {
        let mut app = App::new();
        assert!(!app.is_busy());
        app.apply(HarnessEvent::StatusChanged {
            cursor: Cursor(1),
            status: AgentStatus::Working {
                label: "Thinking".into(),
            },
        });
        assert!(app.is_busy());
    }

    #[test]
    fn an_error_event_becomes_a_visible_transcript_entry() {
        let app = app_with(vec![HarnessEvent::Error {
            cursor: Cursor(1),
            class: axum_proto::ErrorClass::Overload,
            message: "busy".into(),
        }]);
        match &app.entries()[0] {
            Entry::Assistant {
                stop_reason, error, ..
            } => {
                assert_eq!(*stop_reason, Some(StopReason::Error));
                assert!(error.as_deref().unwrap_or_default().contains("busy"));
            }
            other => panic!("expected an assistant entry, got {other:?}"),
        }
    }
}

mod usage_tests {
    use super::super::*;
    use axum_proto::{ModelInfo, SessionId, StopReason, Usage};

    fn ended(id: &str, cursor: u64, usage: Usage) -> HarnessEvent {
        HarnessEvent::AssistantEnded {
            cursor: Cursor(cursor),
            id: MessageId::new(id),
            stop_reason: StopReason::EndTurn,
            error: None,
            usage,
        }
    }

    fn spent(input: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
        }
    }

    #[test]
    fn a_turn_reports_what_it_cost() {
        let mut app = App::new();
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(1),
            id: MessageId::new("a1"),
        });
        app.apply(ended("a1", 1, spent(100, 20)));
        assert_eq!(app.usage().input, 100);
        assert_eq!(app.usage().output, 20);
    }

    #[test]
    fn a_reattach_does_not_count_the_same_turn_twice() {
        // The reason this is derived rather than accumulated. A snapshot carries entries that
        // already include their cost, and the replay after it re-sends the events that
        // produced them: a running total adds both and reads high by however much was replayed.
        let mut app = App::new();
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(1),
            id: MessageId::new("a1"),
        });
        app.apply(ended("a1", 1, spent(100, 20)));
        let entries = app.entries().to_vec();

        let mut rejoined = App::new();
        rejoined.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor(1),
            session: SessionId::new("s"),
            entries,
            status: AgentStatus::Idle,
            model: None,
            choices: Vec::new(),
            thinking: String::new(),
        });
        rejoined.apply(ended("a1", 1, spent(100, 20)));
        assert_eq!(rejoined.usage().input, 100, "counted once, not twice");
    }

    #[test]
    fn window_fullness_is_the_last_prompt_not_the_running_total() {
        // An afternoon that spent ten windows' worth is not ten times full.
        let mut app = App::new();
        for (n, cost) in [(1, spent(1000, 10)), (2, spent(1200, 10))] {
            app.apply(HarnessEvent::AssistantStarted {
                cursor: Cursor(n),
                id: MessageId::new(format!("a{n}")),
            });
            app.apply(ended(&format!("a{n}"), n, cost));
        }
        assert_eq!(app.usage().input, 2200, "the session spent both");
        assert_eq!(app.last_prompt_tokens(), 1200, "the window holds the last");
    }

    #[test]
    fn a_turn_that_reported_nothing_does_not_reset_the_gauge() {
        // A refusal costs nothing and is journalled with a zero. Reading it as "the window is
        // empty now" would make the gauge flicker to zero on every error.
        let mut app = App::new();
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(1),
            id: MessageId::new("a1"),
        });
        app.apply(ended("a1", 1, spent(900, 10)));
        app.apply(HarnessEvent::AssistantStarted {
            cursor: Cursor(2),
            id: MessageId::new("a2"),
        });
        app.apply(ended("a2", 2, Usage::default()));
        assert_eq!(app.last_prompt_tokens(), 900);
    }

    #[test]
    fn the_model_comes_from_the_daemon() {
        let mut app = App::new();
        assert!(app.model.is_none(), "nothing is assumed before it says");
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor::ZERO,
            session: SessionId::new("s"),
            entries: Vec::new(),
            status: AgentStatus::Idle,
            model: Some(ModelInfo {
                name: "p/m".into(),
                context_window: 1000,
            }),
            choices: Vec::new(),
            thinking: String::new(),
        });
        assert_eq!(app.model.expect("a model").name, "p/m");
    }
}

mod onboarding_tests {
    use super::super::*;
    use axum_proto::{ModelChoice, ModelInfo, SessionId};

    fn snapshot(model: Option<ModelInfo>, entries: Vec<Entry>) -> HarnessEvent {
        HarnessEvent::SessionSnapshot {
            cursor: Cursor::ZERO,
            session: SessionId::new("s"),
            entries,
            status: AgentStatus::Idle,
            model,
            choices: vec![ModelChoice {
                name: "p/m".into(),
                context_window: 1000,
                requirement: "set P_KEY".into(),
                reasoning: false,
            }],
            thinking: String::new(),
        }
    }

    fn notices(app: &App) -> Vec<String> {
        app.entries()
            .iter()
            .filter_map(|e| match e {
                Entry::Notice { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_session_with_no_model_is_told_where_to_go() {
        // A fresh install points at a model whose key nobody has set, and all it said was
        // `no-model` in a corner of the footer.
        let mut app = App::new();
        app.apply(snapshot(None, Vec::new()));
        assert_eq!(notices(&app).len(), 1);
        assert!(notices(&app)[0].contains("/model"));
    }

    #[test]
    fn a_session_that_already_works_is_left_alone() {
        let mut app = App::new();
        app.apply(snapshot(
            Some(ModelInfo {
                name: "p/m".into(),
                context_window: 1000,
            }),
            Vec::new(),
        ));
        assert!(notices(&app).is_empty());
    }

    #[test]
    fn reattaching_to_a_session_in_progress_does_not_repeat_it() {
        // Reattaching is routine — the UI redials whenever the socket drops — and a session
        // with a transcript is one you have already been told about.
        let mut app = App::new();
        app.apply(snapshot(
            None,
            vec![Entry::User {
                id: MessageId::new("u1"),
                text: "hello".into(),
            }],
        ));
        assert!(notices(&app).is_empty());
    }
}

mod picking {
    use super::super::*;
    use axum_proto::{ModelChoice, ModelInfo, SessionId};

    fn app_with_a_reasoning_model() -> App {
        let mut app = App::new();
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor::ZERO,
            session: SessionId::new("s"),
            entries: Vec::new(),
            status: AgentStatus::Idle,
            model: Some(ModelInfo {
                name: "p/m".into(),
                context_window: 1000,
            }),
            choices: vec![ModelChoice {
                name: "p/m".into(),
                context_window: 1000,
                requirement: String::new(),
                reasoning: true,
            }],
            thinking: "off".into(),
        });
        app
    }

    #[test]
    fn each_list_records_what_it_is_choosing() {
        // Without this every list's answer went to the same place, and picking a thinking
        // level asked the daemon for a model called "medium".
        let mut app = app_with_a_reasoning_model();
        app.open_model_picker();
        assert_eq!(app.picking, Some(Picking::Model));
        app.open_thinking_picker();
        assert_eq!(app.picking, Some(Picking::Thinking));
    }

    #[test]
    fn a_model_that_reasons_is_offered_every_level() {
        let mut app = app_with_a_reasoning_model();
        app.open_thinking_picker();
        let picker = app.picker.as_ref().expect("a list");
        assert!(
            picker.choices.iter().all(|c| c.ready),
            "{:?}",
            picker.choices
        );
    }

    #[test]
    fn a_model_that_does_not_reason_is_offered_only_off() {
        // Shown and refused rather than hidden, for the same reason an unconfigured provider
        // is: the answer to "how much reasoning" is "this one cannot", not an empty list.
        let mut app = App::new();
        app.apply(HarnessEvent::SessionSnapshot {
            cursor: Cursor::ZERO,
            session: SessionId::new("s"),
            entries: Vec::new(),
            status: AgentStatus::Idle,
            model: Some(ModelInfo {
                name: "p/plain".into(),
                context_window: 1000,
            }),
            choices: vec![ModelChoice {
                name: "p/plain".into(),
                context_window: 1000,
                requirement: String::new(),
                reasoning: false,
            }],
            thinking: "off".into(),
        });
        app.open_thinking_picker();
        let picker = app.picker.as_ref().expect("a list");
        let ready: Vec<&str> = picker
            .choices
            .iter()
            .filter(|c| c.ready)
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(ready, vec!["off"]);
    }

    #[test]
    fn the_list_opens_on_the_level_in_force() {
        let mut app = app_with_a_reasoning_model();
        app.thinking = "high".into();
        app.open_thinking_picker();
        let picker = app.picker.as_ref().expect("a list");
        assert_eq!(picker.current().expect("a row").value, "high");
    }
}
