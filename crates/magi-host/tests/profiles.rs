//! Live mid-session profile switching over the host socket.

use magi_host::catalog::Catalog;
use magi_host::profiles::Profile;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::ask::Card;
use magi_proto::judging::Mode;
use magi_proto::permit::{Grant, Scope};
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use magi_tools::ToolFilter;
use std::collections::BTreeSet;
use std::time::Duration;

fn card(id: &str, reasons: bool) -> Card {
    Card {
        display_name: None,
        vision: false,
        id: id.to_owned(),
        provider: "fake".to_owned(),
        name: id.rsplit('/').next().unwrap_or(id).to_owned(),
        api: "openai-completions".to_owned(),
        context_window: Some(32_000),
        max_output: Some(4000),
        pricing: None,
        popularity_rank: None,
        capabilities: None,
        reasons,
        ready: true,
        setup: None,
        needs: None,
    }
}

#[tokio::test]
async fn set_profile_switches_model_mode_overlay_and_resets_to_default() {
    let dir = magi_model::scratch::Scratch::new("magi-host-profiles", "live-switch");
    let socket = dir.join("session.sock");
    let listener = magi_ipc::bind(&socket).await.expect("bind");

    let mut catalog = Catalog::empty();
    catalog.cwd = dir.to_path_buf();
    catalog.cards = vec![
        card("fake/base", false),
        card("fake/plan", true),
        card("fake/exec", true),
    ];
    catalog.chosen = Some("fake/base".into());
    catalog.base_model = Some("fake/base".into());
    catalog.mode = Mode::Ask;
    catalog.rules.deny = vec![Grant {
        verb: "run".into(),
        scope: Scope::Program {
            program: "rm".into(),
        },
    }];
    catalog.profiles = vec![
        Profile {
            name: "planner".into(),
            description: Some("Plans only".into()),
            prompt: Some("Do not edit.".into()),
            model: Some("fake/plan".into()),
            thinking: Some("high".into()),
            mode: Some(Mode::Ask),
            deny: vec![Grant {
                verb: "write".into(),
                scope: Scope::Anything,
            }],
            tools: ToolFilter::Deny(BTreeSet::from(["write".into(), "edit".into()])),
            agents: Some(vec!["explorer".into()]),
            ..Profile::default()
        },
        Profile {
            name: "executor".into(),
            description: Some("Builds fast".into()),
            model: Some("fake/exec".into()),
            mode: Some(Mode::Edits),
            ..Profile::default()
        },
        Profile {
            name: "neutral".into(),
            description: Some("Inherits base model and mode; clamps thinking".into()),
            thinking: Some("max".into()),
            ..Profile::default()
        },
    ];
    let backend = catalog.backend("fake/base");
    let session = magi_host::open_session(1, "profiles");
    let server = tokio::spawn(async move {
        let _ = magi_host::serve_catalog(listener, session, backend, catalog).await;
    });

    let stream = magi_ipc::connect(&socket).await.expect("connect");
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
            key: magi_ipc::read_key(&socket),
        })
        .await
        .expect("attach");

    // Snapshot followed immediately by ProfileChanged listing both profiles.
    let first: HarnessEvent = reader.read().await.expect("snapshot");
    assert!(matches!(first, HarnessEvent::SessionSnapshot { .. }));
    let second: HarnessEvent = reader.read().await.expect("initial profiles");
    match second {
        HarnessEvent::ProfileChanged {
            profile, profiles, ..
        } => {
            assert_eq!(profile, None);
            let names: Vec<&str> = profiles.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(names, vec!["planner", "executor", "neutral"]);
        }
        other => panic!("expected ProfileChanged, got {other:?}"),
    }

    // Switch to planner: model -> fake/plan, thinking -> high, write denied in judging.
    writer
        .write(&UiCommand::SetProfile {
            name: "planner".into(),
        })
        .await
        .expect("set planner");
    let mut saw_mode = None;
    let mut saw_model = None;
    let mut saw_profile = None;
    for _ in 0..4 {
        let ev: HarnessEvent = tokio::time::timeout(Duration::from_secs(5), reader.read())
            .await
            .expect("timeout")
            .expect("event");
        match ev {
            HarnessEvent::ModeChanged { judging, .. } => saw_mode = Some(judging),
            HarnessEvent::ModelChanged {
                model, thinking, ..
            } => saw_model = Some((model.map(|m| m.name), thinking)),
            HarnessEvent::ProfileChanged { profile, .. } => saw_profile = Some(profile),
            HarnessEvent::Noticed { .. } => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }
    let judging = saw_mode.expect("mode");
    assert_eq!(judging.mode, Mode::Ask);
    assert!(
        judging.denied.iter().any(|r| r.contains("write")),
        "planner denies write: {:?}",
        judging.denied
    );
    assert!(
        judging.denied.iter().any(|r| r.contains("rm")),
        "global deny preserved: {:?}",
        judging.denied
    );
    assert_eq!(
        saw_model,
        Some((Some("fake/plan".into()), Some("high".into())))
    );
    assert_eq!(saw_profile, Some(Some("planner".into())));

    // Switch to executor: mode -> edits, planner's write deny vanishes, global rm deny remains.
    writer
        .write(&UiCommand::SetProfile {
            name: "executor".into(),
        })
        .await
        .expect("set executor");
    let mut exec_mode = None;
    let mut exec_model = None;
    for _ in 0..4 {
        let ev: HarnessEvent = tokio::time::timeout(Duration::from_secs(5), reader.read())
            .await
            .expect("timeout")
            .expect("event");
        match ev {
            HarnessEvent::ModeChanged { judging, .. } => exec_mode = Some(judging),
            HarnessEvent::ModelChanged { model, .. } => exec_model = model.map(|m| m.name),
            HarnessEvent::ProfileChanged { .. } | HarnessEvent::Noticed { .. } => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }
    let exec_judging = exec_mode.expect("exec mode");
    assert_eq!(exec_judging.mode, Mode::Edits);
    assert!(!exec_judging.denied.iter().any(|r| r.contains("write")));
    assert!(exec_judging.denied.iter().any(|r| r.contains("rm")));
    assert_eq!(exec_model.as_deref(), Some("fake/exec"));

    // Switching from executor (mode=edits, model=fake/exec) to neutral (omits mode & model,
    // requests thinking=max on non-reasoning fake/base) must restore base mode & base model
    // and clamp thinking cleanly to "off" without leaking executor's overrides.
    writer
        .write(&UiCommand::SetProfile {
            name: "neutral".into(),
        })
        .await
        .expect("set neutral");
    let mut neutral_mode = None;
    let mut neutral_model = None;
    for _ in 0..4 {
        let ev: HarnessEvent = tokio::time::timeout(Duration::from_secs(5), reader.read())
            .await
            .expect("timeout")
            .expect("event");
        match ev {
            HarnessEvent::ModeChanged { judging, .. } => neutral_mode = Some(judging.mode),
            HarnessEvent::ModelChanged {
                model, thinking, ..
            } => neutral_model = Some((model.map(|m| m.name), thinking)),
            HarnessEvent::ProfileChanged { .. } | HarnessEvent::Noticed { .. } => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert_eq!(
        neutral_mode,
        Some(Mode::Ask),
        "must not leak executor edits mode"
    );
    assert_eq!(
        neutral_model,
        Some((Some("fake/base".into()), Some("off".into()))),
        "must restore base model and clamp unsupported max thinking to off"
    );

    // Unknown profile is refused cleanly without changing active profile.
    writer
        .write(&UiCommand::SetProfile {
            name: "ghost".into(),
        })
        .await
        .expect("set ghost");
    let refused: HarnessEvent = tokio::time::timeout(Duration::from_secs(5), reader.read())
        .await
        .expect("timeout")
        .expect("refused");
    assert!(
        matches!(&refused, HarnessEvent::Refused { message, .. } if message.contains("ghost")),
        "{refused:?}"
    );

    // Clearing back to "default" restores base model and mode.
    writer
        .write(&UiCommand::SetProfile {
            name: "default".into(),
        })
        .await
        .expect("set default");
    let mut back_profile = Some(Some("executor".to_owned()));
    let mut back_model = None;
    for _ in 0..4 {
        let ev: HarnessEvent = tokio::time::timeout(Duration::from_secs(5), reader.read())
            .await
            .expect("timeout")
            .expect("event");
        match ev {
            HarnessEvent::ProfileChanged { profile, .. } => back_profile = Some(profile),
            HarnessEvent::ModelChanged { model, .. } => back_model = model.map(|m| m.name),
            HarnessEvent::ModeChanged { .. } | HarnessEvent::Noticed { .. } => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert_eq!(back_profile, Some(None));
    assert_eq!(back_model.as_deref(), Some("fake/base"));

    server.abort();
}
