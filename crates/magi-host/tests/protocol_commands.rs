#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_proto::authentication::{Reply, Request};
use magi_proto::{Environment, UiCommand, permit};

#[test]
fn a_verb_is_the_tag_the_command_travels_under() {
    for command in [
        UiCommand::Interrupt,
        UiCommand::DeclareNeeds,
        UiCommand::BlockSized { rows: 8, cols: 40 },
        UiCommand::SetMode { mode: "x".into() },
        UiCommand::SelectModel {
            request: 7,
            name: "fake/one".into(),
        },
        UiCommand::TakeGrants { grants: Vec::new() },
        UiCommand::Inspect,
        UiCommand::AttachLatest {
            draws: false,
            key: None,
        },
        UiCommand::Environment(Environment {
            source: false,
            input: "A=1".into(),
        }),
    ] {
        let wire = serde_json::to_value(&command).expect("encode");
        assert_eq!(wire["command"], command.verb(), "{command:?}");
    }
}

#[test]
fn a_watcher_may_attach_and_leave_and_nothing_else() {
    assert!(UiCommand::Detach.watches());
    assert!(!UiCommand::Interrupt.watches());
    assert!(
        !UiCommand::SelectModel {
            request: 7,
            name: "fake/one".into()
        }
        .watches()
    );
    assert!(
        !UiCommand::SetMode {
            mode: "auto".into()
        }
        .watches()
    );
}

#[test]
fn a_watcher_may_look_but_not_review_or_change_the_environment() {
    assert!(UiCommand::Inspect.watches());
    assert!(
        UiCommand::AttachLatest {
            draws: true,
            key: None
        }
        .watches()
    );
    assert!(
        !UiCommand::Review {
            tool: "read".into(),
            action: permit::Action::Read { path: "/".into() },
            key: None,
        }
        .watches()
    );
    assert!(
        !UiCommand::Environment(Environment {
            source: true,
            input: "PATH=/x".into(),
        })
        .watches()
    );
}

#[test]
fn secret_requests_are_redacted_but_transport_round_trips() {
    let command = UiCommand::Authenticate {
        request: 7,
        action: Request::SetKey {
            provider: "mistral".into(),
            key: "private-sentinel".into(),
        },
    };
    assert!(!format!("{command:?}").contains("private-sentinel"));
    assert!(!command.watches());
    let bytes = serde_json::to_vec(&command).expect("encode");
    assert_eq!(
        serde_json::from_slice::<UiCommand>(&bytes).expect("decode"),
        command
    );
    let event = magi_proto::HarnessEvent::Authentication {
        request: 7,
        reply: Reply::failed("cancelled"),
    };
    assert_eq!(event.cursor(), magi_proto::Cursor::ZERO);
}
