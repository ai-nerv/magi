use magi_host::{catalog::Catalog, session::Session, turn::Backend};
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{AgentStatus, Cursor, HarnessEvent, SessionId, UiCommand};
use magi_testkit::{Mind, mind};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

struct Fixture {
    dir: Scratch,
    revision: String,
    serving: tokio::task::JoinHandle<Result<(), magi_host::HostError>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

async fn fixture(name: &str, mind: &Mind, child: bool, broken: bool) -> Fixture {
    let dir = Scratch::new("mr", name);
    let body = "Verified finding: the report reached the model without a tool call.";
    let digest = format!("{:x}", Sha256::digest(body.as_bytes()));
    let revision = "a".repeat(64);
    std::fs::write(
        dir.join("report.json"),
        serde_json::json!({
            "report": body, "revision": revision, "digest": digest,
        })
        .to_string(),
    )
    .expect("fixture operation succeeds");
    let coord = dir.join("coord");
    std::fs::write(&coord, "#!/bin/sh\ncase \"$*\" in\n *--who=*) cat \"$REPORT_JSON\";;\n *) cat > \"$PUBLISHED\"; echo 'Handed in';;\nesac\n").expect("fixture operation succeeds");
    std::fs::set_permissions(&coord, std::fs::Permissions::from_mode(0o755))
        .expect("fixture operation succeeds");
    let mut environ = std::collections::BTreeMap::from([
        ("MAGI_COORD_PROGRAM".into(), coord.display().to_string()),
        (
            "REPORT_JSON".into(),
            dir.join("report.json").display().to_string(),
        ),
        (
            "PUBLISHED".into(),
            dir.join("published").display().to_string(),
        ),
    ]);
    if child {
        environ.insert("MAGI_MELCHIOR_PARENT".into(), "parent".into());
        environ.insert("MAGI_MELCHIOR_ID".into(), "child".into());
    }
    let backend = Backend {
        tools: if broken {
            vec![("broken".into(), "not valid lua".into())]
        } else if name == "explicit" || name == "toolerror" {
            vec![(
                "agent".into(),
                format!(
                    r#"magi.tool("agent", {{
              description = "Hand in a report", parameters = {{type = "object"}},
              transport = {{kind = "lua"}}, run = function() return {{content = "fixture tool result", is_error = {}}} end
            }})"#,
                    name == "toolerror"
                ),
            )]
        } else {
            Vec::new()
        },
        clients: Vec::new(),
        tooling: Default::default(),
        cwd: dir.to_path_buf(),
        model: "fake/one".into(),
        mind: mind.program().display().to_string(),
        wants: Default::default(),
        context_window: Some(200_000),
        max_output: None,
        system: None,
        confine: false,
        isolate: false,
        grants: Vec::new(),
        environ,
        helpers: Default::default(),
        deciders: Vec::new(),
    };
    let listener = magi_ipc::bind(&dir.join("s.sock"))
        .await
        .expect("fixture operation succeeds");
    let serving = tokio::spawn(magi_host::serve_on(
        listener,
        Session::recorded(SessionId::new(name), Vec::new()),
        Some(backend),
        Catalog::empty(),
        Some(dir.join("no-memory.sock")),
    ));
    Fixture {
        dir,
        revision,
        serving,
    }
}

async fn attach(fixture: &Fixture) -> (Reader, Writer) {
    let stream = magi_ipc::connect(&fixture.dir.join("s.sock"))
        .await
        .expect("fixture operation succeeds");
    let (read, write) = stream.into_split();
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor(0),
            draws: false,
        })
        .await
        .expect("fixture operation succeeds");
    let mut reader = FrameReader::new(read);
    let _: HarnessEvent = reader.read().await.expect("fixture operation succeeds");
    (reader, writer)
}

async fn until(reader: &mut Reader, matches: impl Fn(&HarnessEvent) -> bool) -> HarnessEvent {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let event = reader.read().await.expect("fixture operation succeeds");
            if matches(&event) {
                return event;
            }
        }
    })
    .await
    .expect("event barrier")
}

async fn prefix(reader: &mut Reader, nth: usize) {
    let part = format!("part-{}", nth - 1);
    until(
        reader,
        |event| matches!(event, HarnessEvent::AssistantDelta { text, .. } if text == &part),
    )
    .await;
}

async fn idle(reader: &mut Reader) {
    until(reader, |event| {
        matches!(
            event,
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
}

async fn prompt(writer: &mut Writer) {
    writer
        .write(&UiCommand::SubmitPrompt {
            text: "scan and report".into(),
            aside: String::new(),
        })
        .await
        .expect("fixture operation succeeds");
}

async fn report(writer: &mut Writer, fixture: &Fixture, who: &str) {
    writer
        .write(&UiCommand::Arrived {
            who: who.into(),
            kin: "child".into(),
            sort: "report".into(),
            text: serde_json::json!({"revision": fixture.revision}).to_string(),
        })
        .await
        .expect("fixture operation succeeds");
}

#[tokio::test]
async fn a_report_after_escape_restarts_the_parent_and_is_loaded_automatically() {
    let mind = Mind::controlled("report-after-escape");
    let fixture = fixture("after", &mind, false, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    prefix(&mut reader, 1).await;
    writer
        .write(&UiCommand::Interrupt)
        .await
        .expect("fixture operation succeeds");
    idle(&mut reader).await;
    report(&mut writer, &fixture, "demo/main/child").await;
    prefix(&mut reader, 2).await;
    assert!(mind.asks()[1].contains("Verified finding"));
    mind.release(1);
    idle(&mut reader).await;
    report(&mut writer, &fixture, "demo/main/child").await;
    until(&mut reader, |event| {
        matches!(event, HarnessEvent::MessageArrived { .. })
    })
    .await;
    idle(&mut reader).await;
    assert_eq!(
        mind.asked(),
        2,
        "handled report duplicates must not start another turn"
    );
}

#[tokio::test]
async fn escape_during_report_handling_retriggers_without_another_notification() {
    let mind = Mind::controlled("report-retrigger");
    let fixture = fixture("retry", &mind, false, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    report(&mut writer, &fixture, "demo/main/child").await;
    prefix(&mut reader, 1).await;
    writer
        .write(&UiCommand::Interrupt)
        .await
        .expect("fixture operation succeeds");
    prefix(&mut reader, 2).await;
    assert!(mind.asks()[1].contains("Verified finding"));
    assert!(mind.asks()[1].contains("reports remain unacknowledged"));
    mind.release(1);
    idle(&mut reader).await;
    assert_eq!(mind.asked(), 2);
    assert!(
        !mind.overlapped(),
        "retry must wait for the interrupted turn to stop"
    );
}

#[tokio::test]
async fn several_reports_survive_interrupting_the_parent() {
    let mind = Mind::controlled("report-many");
    let fixture = fixture("many", &mind, false, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    prefix(&mut reader, 1).await;
    report(&mut writer, &fixture, "demo/main/a").await;
    report(&mut writer, &fixture, "demo/main/b").await;
    writer
        .write(&UiCommand::Interrupt)
        .await
        .expect("fixture operation succeeds");
    prefix(&mut reader, 2).await;
    let ask = &mind.asks()[1];
    assert!(
        ask.contains("subagent report/a") && ask.contains("subagent report/b"),
        "{ask}"
    );
    mind.release(1);
    idle(&mut reader).await;
    assert_eq!(mind.asked(), 2);
}

#[tokio::test]
async fn a_child_with_an_empty_answer_still_publishes_a_report() {
    let mind = Mind::answering("report-empty", "");
    let fixture = fixture("empty", &mind, true, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    idle(&mut reader).await;
    let published =
        std::fs::read_to_string(fixture.dir.join("published")).expect("fixture operation succeeds");
    assert!(published.contains("Outcome: completed"), "{published}");
    assert!(
        published.contains("No findings or final answer"),
        "{published}"
    );
}

#[tokio::test]
async fn a_child_with_a_broken_worker_still_publishes_a_failure_report() {
    let mind = Mind::answering("report-broken", "unused");
    let fixture = fixture("broken", &mind, true, true).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    idle(&mut reader).await;
    let published =
        std::fs::read_to_string(fixture.dir.join("published")).expect("fixture operation succeeds");
    assert!(published.contains("Outcome: failed"), "{published}");
    assert!(published.contains("Error:"), "{published}");
    assert_eq!(mind.asked(), 0);
}

#[tokio::test]
async fn a_provider_error_becomes_a_failure_report_not_a_success() {
    let line = mind::failed_line("fixture provider refused", "invalid");
    let mind = Mind::saying("report-error", &[&line]);
    let fixture = fixture("error", &mind, true, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    idle(&mut reader).await;
    let published =
        std::fs::read_to_string(fixture.dir.join("published")).expect("fixture operation succeeds");
    assert!(published.contains("Outcome: failed"), "{published}");
    assert!(
        published.contains("fixture provider refused"),
        "{published}"
    );
}

#[tokio::test]
async fn an_interrupted_child_reports_the_interruption_and_partial_findings() {
    let mind = Mind::controlled("report-child-interrupt");
    let fixture = fixture("stopped", &mind, true, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    prefix(&mut reader, 1).await;
    writer
        .write(&UiCommand::Interrupt)
        .await
        .expect("fixture operation succeeds");
    idle(&mut reader).await;
    let published =
        std::fs::read_to_string(fixture.dir.join("published")).expect("fixture operation succeeds");
    assert!(published.contains("Outcome: interrupted"), "{published}");
    assert!(published.contains("part-0"), "{published}");
}

#[tokio::test]
async fn an_explicit_report_is_preserved_and_finalized_at_turn_end() {
    let calls = mind::call_lines(
        "r",
        "agent",
        r#"{"verb":"report","message":"Full explicit report"}"#,
    );
    let first: Vec<&str> = calls.iter().map(String::as_str).collect();
    let final_text = mind::text_line("Report submitted");
    let final_stop = mind::stop_line();
    let mind = Mind::turns("report-explicit", &[&first, &[&final_text, &final_stop]]);
    let fixture = fixture("explicit", &mind, true, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    idle(&mut reader).await;
    let published =
        std::fs::read_to_string(fixture.dir.join("published")).expect("fixture operation succeeds");
    assert!(published.contains("Outcome: completed"), "{published}");
    assert!(
        published.contains("Verified finding"),
        "the stored report was lost: {published}"
    );
    assert!(published.contains("Report submitted"), "{published}");
}

#[tokio::test]
async fn tool_errors_are_included_even_when_the_child_finishes_normally() {
    let calls = mind::call_lines("r", "agent", r#"{"verb":"status"}"#);
    let first: Vec<&str> = calls.iter().map(String::as_str).collect();
    let stop = mind::stop_line();
    let mind = Mind::turns("report-toolerror", &[&first, &[&stop]]);
    let fixture = fixture("toolerror", &mind, true, false).await;
    let (mut reader, mut writer) = attach(&fixture).await;
    prompt(&mut writer).await;
    idle(&mut reader).await;
    let published =
        std::fs::read_to_string(fixture.dir.join("published")).expect("published report");
    assert!(
        published.contains("Outcome: completed with tool errors"),
        "{published}"
    );
    assert!(
        published.contains("agent: fixture tool result"),
        "{published}"
    );
    assert!(
        published.contains("No findings or final answer"),
        "{published}"
    );
}

#[tokio::test]
async fn a_stale_notification_loads_and_acknowledges_the_current_report_revision() {
    let mind = Mind::controlled("report-newer");
    let fixture = fixture("newer", &mind, false, false).await;
    let latest = "b".repeat(64);
    let body = "Newest report: repeated no-findings reports are separate submissions.";
    std::fs::write(
        fixture.dir.join("report.json"),
        serde_json::json!({
            "report": body, "revision": latest,
            "digest": format!("{:x}", Sha256::digest(body.as_bytes())),
        })
        .to_string(),
    )
    .expect("newer report fixture");
    let (mut reader, mut writer) = attach(&fixture).await;
    report(&mut writer, &fixture, "demo/main/child").await;
    prefix(&mut reader, 1).await;
    assert!(mind.asks()[0].contains(body));
    mind.release(0);
    idle(&mut reader).await;
    writer
        .write(&UiCommand::Arrived {
            who: "demo/main/child".into(),
            kin: "child".into(),
            sort: "report".into(),
            text: serde_json::json!({"revision": latest}).to_string(),
        })
        .await
        .expect("latest notification");
    until(&mut reader, |event| {
        matches!(event, HarnessEvent::MessageArrived { .. })
    })
    .await;
    idle(&mut reader).await;
    assert_eq!(
        mind.asked(),
        1,
        "the already loaded newer revision must not be handled twice"
    );
}

#[tokio::test]
async fn long_reports_are_verified_and_trimmed_at_a_utf8_boundary_with_a_read_hint() {
    let mind = Mind::answering("report-long", "Read the remaining pages before concluding.");
    let fixture = fixture("long", &mind, false, false).await;
    let body = format!("{}END-OF-FULL-REPORT", "界".repeat(20_000));
    std::fs::write(
        fixture.dir.join("report.json"),
        serde_json::json!({
            "report": body, "revision": fixture.revision,
            "digest": format!("{:x}", Sha256::digest(body.as_bytes())),
        })
        .to_string(),
    )
    .expect("large UTF-8 report fixture");
    let (mut reader, mut writer) = attach(&fixture).await;
    report(&mut writer, &fixture, "demo/main/child").await;
    idle(&mut reader).await;
    let asks = mind.asks();
    assert_eq!(asks.len(), 1);
    assert!(asks[0].contains("Automatic delivery limit: 39999 of"));
    assert!(asks[0].contains("Read the remainder with agent report"));
    assert!(!asks[0].contains("END-OF-FULL-REPORT"));
}
