#[cfg(test)]
mod contract {
    use super::super::*;
    use crate::catalog::Backend;
    use crate::session::Session;
    use magi_model::{Content, Message};
    use magi_proto::{Cursor, Entry, HarnessEvent, MessageId, SessionId};
    use serde_json::json;

    fn backend() -> Backend {
        let mut catalog = crate::catalog::Catalog::empty();
        catalog.cards.push(
            serde_json::from_value(json!({
                "id":"fake/one", "provider":"fake", "name":"one", "api":"anthropic-messages",
                "ready":true
            }))
            .expect("card"),
        );
        catalog.backend("fake/one").expect("backend")
    }

    fn context() -> Context {
        Context {
            system: Some("private rules".into()),
            messages: vec![Message::user("private question")],
            tools: vec![magi_model::Tool {
                name: "read".into(),
                description: "private description".into(),
                parameters: json!({"type":"object","required":[],"properties":{}}),
            }],
        }
    }

    fn layout(slots: serde_json::Value) -> Layout {
        serde_json::from_value(json!({"slots":slots})).expect("layout")
    }

    #[test]
    fn identical_frames_and_appended_rounds_do_not_invalidate() {
        let mut frames = Frames::default();
        let mut context = context();
        let backend = backend();
        for _ in 0..3 {
            let original = context.clone();
            assert!(
                frames
                    .observe(&context, backend.frame(), None, &[])
                    .is_empty()
            );
            assert_eq!(context, original);
            context.messages.push(Message::assistant("answer"));
            context.messages.push(Message::user("next"));
        }
    }

    #[test]
    fn schemas_order_model_system_and_options_have_separate_reasons() {
        let base = context();
        let base_backend = backend();
        for (change, reason) in [
            (0, Invalidation::ToolsChanged),
            (1, Invalidation::ToolsChanged),
            (2, Invalidation::ModelChanged),
            (3, Invalidation::ModelChanged),
            (4, Invalidation::RulesChanged),
            (5, Invalidation::OptionsChanged),
        ] {
            let mut frames = Frames::default();
            assert!(
                frames
                    .observe(&base, base_backend.frame(), None, &[])
                    .is_empty()
            );
            let mut next = base.clone();
            let mut backend = base_backend.clone();
            match change {
                0 => next.tools[0].parameters["required"] = json!(["path"]),
                1 => next.tools.insert(
                    0,
                    magi_model::Tool {
                        name: "activated".into(),
                        ..next.tools[0].clone()
                    },
                ),
                2 => backend.model = "fake/two".into(),
                3 => backend.mind = "another-melchior".into(),
                4 => next.system = Some("fresh rules".into()),
                _ => backend.wants.provider = Some("explicit preference".into()),
            }
            assert_eq!(frames.observe(&next, backend.frame(), None, &[]), [reason]);
            assert!(frames.observe(&next, backend.frame(), None, &[]).is_empty());
        }
    }

    #[test]
    fn layout_metadata_is_not_content_but_rules_and_summaries_are() {
        let backend = backend();
        let session = Session::recorded(SessionId::new("test"), vec![]);
        let mut first = layout(json!([{"kind":"rules","text":"old"}]));
        let mut frames = Frames::default();
        let context = crate::laying::render(&session, &first);
        assert!(
            frames
                .observe(&context, backend.frame(), Some(&first), &[])
                .is_empty()
        );
        first.id = "new request id".into();
        first.budget = json!({"tokens":900});
        first.why = "different explanation".into();
        assert!(
            frames
                .observe(&context, backend.frame(), Some(&first), &[])
                .is_empty()
        );
        let next = layout(json!([{"kind":"rules","text":"fresh"}]));
        let rendered = crate::laying::render(&session, &next);
        assert_eq!(
            frames.observe(&rendered, backend.frame(), Some(&next), &[]),
            [Invalidation::RulesChanged, Invalidation::LayoutChanged]
        );
        let summary = layout(json!([
            {"kind":"rules","text":"fresh"}, {"kind":"summary","text":"summary"}
        ]));
        let rendered = crate::laying::render(&session, &summary);
        assert_eq!(
            frames.observe(&rendered, backend.frame(), Some(&summary), &[]),
            [Invalidation::SummaryActivated]
        );
        assert!(
            frames
                .observe(&rendered, backend.frame(), Some(&summary), &[])
                .is_empty()
        );
    }

    #[test]
    fn masks_drops_reordering_images_and_opaque_signatures_are_visible() {
        let backend = backend();
        let mut original = context();
        original.messages.push(Message::assistant("answer"));
        original.messages.push(Message::user("next"));
        for change in 0..5 {
            let mut frames = Frames::default();
            frames.observe(&original, backend.frame(), None, &[]);
            let mut next = original.clone();
            let stubs = layout(json!([{"kind":"stub","cursor":1,"text":"masked"}]));
            let (selected, expected) = match change {
                0 => {
                    next.messages.remove(0);
                    (None, Invalidation::LayoutReduced)
                }
                1 => {
                    next.messages.swap(0, 1);
                    (None, Invalidation::LayoutChanged)
                }
                2 => {
                    next.messages[0].content.push(Content::Image {
                        data: "image bytes".into(),
                        media_type: "image/png".into(),
                    });
                    (None, Invalidation::LayoutChanged)
                }
                3 => {
                    next.messages[1].content.push(Content::Thinking {
                        thinking: "private".into(),
                        signature: Some("opaque".into()),
                    });
                    (None, Invalidation::LayoutChanged)
                }
                _ => {
                    next.messages[0] = Message::user("masked");
                    (Some(&stubs), Invalidation::LayoutChanged)
                }
            };
            let exact = next.clone();
            assert_eq!(
                frames.observe(&next, backend.frame(), selected, &[]),
                [expected]
            );
            assert_eq!(next, exact);
        }
    }

    #[test]
    fn observer_is_fixed_size_and_diagnostics_never_contain_content_or_hashes() {
        assert!(!std::mem::needs_drop::<Frames>());
        assert!(std::mem::size_of::<Frames>() <= 384);
        let backend = backend();
        let mut session = Session::recorded(SessionId::new("private-session"), vec![]);
        let mut events = session.subscribe();
        let mut context = context();
        session.report_frame(&context, backend.frame());
        context.system = Some("secret replacement".into());
        session.report_frame(&context, backend.frame());
        let first = events.try_recv().expect("baseline");
        let second = events.try_recv().expect("transition");
        assert_eq!(first.cursor(), Cursor::ZERO);
        assert_eq!(
            serde_json::to_value(first).expect("event"),
            json!({"event":"cache_frame","reasons":[]})
        );
        assert_eq!(
            serde_json::to_value(second).expect("event"),
            json!({"event":"cache_frame","reasons":["rules_changed"]})
        );
        assert!(session.entries().is_empty());
        assert!(!session.has_pending());
    }

    #[test]
    fn restored_and_replaced_sessions_report_unknown_compatibility_once() {
        let backend = backend();
        let context = context();
        let entry = Entry::User {
            id: MessageId::new("u"),
            text: "previous".into(),
            aside: String::new(),
            images: vec![],
        };
        let recorded = Session::recorded(SessionId::new("recorded"), vec![entry.clone()]);
        let restored =
            Session::restored(SessionId::new("restored"), vec![(Cursor(9), entry.clone())])
                .expect("restore");
        let mut replaced = Session::recorded(SessionId::new("before"), vec![]);
        replaced.report_frame(&context, backend.frame());
        replaced.resume_prepared(magi_journal::Journal::recorded(
            SessionId::new("after"),
            vec![entry],
        ));
        for mut session in [recorded, restored, replaced] {
            let mut events = session.subscribe();
            session.report_frame(&context, backend.frame());
            session.report_frame(&context, backend.frame());
            assert_eq!(
                events.try_recv().expect("resumed"),
                HarnessEvent::CacheFrame {
                    reasons: vec![Invalidation::ResumeIncompatible]
                }
            );
            assert_eq!(
                events.try_recv().expect("ordinary"),
                HarnessEvent::CacheFrame { reasons: vec![] }
            );
        }
    }

    #[test]
    fn empty_startup_has_no_previous_frame_but_explicit_replacement_does() {
        let backend = backend();
        let context = context();
        let mut session = Session::restored(SessionId::new("fresh"), vec![]).expect("startup");
        let mut events = session.subscribe();
        session.report_frame(&context, backend.frame());
        assert_eq!(
            events.try_recv().expect("baseline"),
            HarnessEvent::CacheFrame { reasons: vec![] }
        );
        session.resume_prepared(magi_journal::Journal::recorded(
            SessionId::new("empty replacement"),
            vec![],
        ));
        assert!(matches!(
            events.try_recv().expect("snapshot"),
            HarnessEvent::SessionSnapshot { .. }
        ));
        session.report_frame(&context, backend.frame());
        assert_eq!(
            events.try_recv().expect("replacement"),
            HarnessEvent::CacheFrame {
                reasons: vec![Invalidation::ResumeIncompatible]
            }
        );
    }
}
