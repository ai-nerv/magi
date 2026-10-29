use super::*;
use magi_testkit::Mind;

const CASE: &str =
    "models::successful_model_replacement_cancels_preparation_and_preserves_frozen_context";

#[test]
fn successful_model_replacement_cancels_preparation_and_preserves_frozen_context() {
    if std::env::var(authority::CHILD).as_deref() == Ok(CASE) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(checked());
        return;
    }
    let (Some(casper), Some(memory), Some(tools)) = (
        selected("CASPER_TEST_BINARY"),
        selected("BALTHASAR_TEST_BINARY"),
        selected("CASPER_TEST_TOOLS"),
    ) else {
        return;
    };
    authority::isolated(CASE, &casper, &memory, &tools);
}

fn source(body: &str) -> String {
    format!("---\nname: review\ndescription: Model replacement\n---\n{body}\n")
}

async fn idle(reader: &mut Reader) {
    event(reader, |event| {
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

async fn checked() {
    let mind = Mind::answering("workflow-model-swap", "synthetic answer");
    let fixture = Fixture::configured("model-ok", false, |catalog| {
        let bin = catalog.cwd.join("bin");
        std::fs::create_dir(&bin).expect("utility directory");
        for name in ["cat", "dirname", "sh"] {
            std::os::unix::fs::symlink(utility(name), bin.join(name)).expect("utility");
        }
        catalog.environ.insert("PATH".into(), bin.display().to_string());
        let quote = |path: &Path| magi_proto::permit::quoted(path.to_str().expect("fixture path"));
        let script = format!(
            "#!{}\nif [ \"$1\" = workflows ] && [ -f {} ] && {} {} 2>/dev/null; then\nprintf '%s' \"$$\" > {}\nexec {} 30\nfi\nexec {} \"$@\"\n",
            utility("sh").display(),
            quote(&catalog.cwd.join("arm")),
            quote(&utility("mkdir")),
            quote(&catalog.cwd.join("gate")),
            quote(&catalog.cwd.join("pid")),
            quote(&utility("sleep")),
            magi_proto::permit::quoted(&catalog.tooling.program),
        );
        let program = catalog.cwd.join("gated-reader");
        std::fs::write(&program, &script).expect("helper");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o500)).expect("mode");
        catalog.tooling = Tooling::of(program.to_str().expect("helper path"));
        catalog.tooling.pin = Some(format!("{:x}", Sha256::digest(script.as_bytes())));
        catalog.mind = mind.program().display().to_string();
        catalog.chosen = Some("fake/one".into());
        catalog.cards = ["one", "two"].into_iter().map(|name| {
            serde_json::from_value(serde_json::json!({
                "id":format!("fake/{name}"),"provider":"fake","name":name,
                "api":"anthropic-messages","context_window":200000,"max_output":4096,"ready":true
            })).expect("synthetic model")
        }).collect();
        assert!(catalog.helpers.roles.is_empty());
    }).await.expect("native fixture");
    let path = fixture.dir.join(".magi/skills/review/SKILL.md");
    let original = source("FROZEN-BEFORE-SWITCH");
    put(&path, &original);
    let (mut reader, mut writer, _) = fixture.attach().await;
    prompt(&mut writer, "/skill review first").await;
    event(&mut reader, |event| {
        matches!(event, HarnessEvent::WorkflowMessage { .. })
    })
    .await;
    idle(&mut reader).await;
    assert_eq!(mind.asked(), 1);
    let (_, _, initial) = fixture.attach().await;
    let frozen = initial
        .iter()
        .find(|entry| matches!(entry, Entry::Workflow { .. }))
        .expect("accepted workflow")
        .clone();
    assert!(matches!(&frozen, Entry::Workflow { workflow, .. }
        if workflow.body == "FROZEN-BEFORE-SWITCH\n"
        && workflow.digest == format!("{:x}", Sha256::digest(original.as_bytes()))));

    put(&fixture.dir.join("arm"), "armed");
    prompt(&mut writer, "/skill review cancelled-generation").await;
    let pid = started(&fixture.dir.join("pid")).await;
    writer
        .write(&UiCommand::SetModel {
            name: "fake/two".into(),
        })
        .await
        .expect("switch model");
    reaped(pid).await;
    let (mut changed, mut cancelled) = (false, false);
    while !changed || !cancelled {
        match event(&mut reader, |event| {
            matches!(
                event,
                HarnessEvent::ModelChanged { .. }
                    | HarnessEvent::Refused { .. }
                    | HarnessEvent::WorkflowMessage { .. }
            )
        })
        .await
        {
            HarnessEvent::ModelChanged { model, .. } => {
                assert!(!changed);
                assert_eq!(model.expect("selected model").name, "fake/two");
                changed = true;
            }
            HarnessEvent::Refused { message, .. } => {
                assert!(!cancelled);
                assert!(message.contains("preparation"), "{message}");
                cancelled = true;
            }
            other => panic!("unexpected switch event: {other:?}"),
        }
    }
    assert_eq!(
        mind.asked(),
        1,
        "cancelled preparation must not reach a model"
    );
    std::fs::remove_file(&path).expect("delete source");
    prompt(&mut writer, "Continue using the frozen selection").await;
    event(&mut reader, |event| {
        matches!(event, HarnessEvent::UserMessage { .. })
    })
    .await;
    idle(&mut reader).await;
    assert_eq!(mind.asked(), 2);

    put(&path, &source("NEW-AFTER-SWITCH"));
    prompt(&mut writer, "/skill review fresh").await;
    let accepted = event(&mut reader, |event| {
        matches!(
            event,
            HarnessEvent::WorkflowMessage { .. } | HarnessEvent::Refused { .. }
        )
    })
    .await;
    assert!(
        matches!(accepted, HarnessEvent::WorkflowMessage { workflow, .. }
        if workflow.body == "NEW-AFTER-SWITCH\n" && workflow.arguments == "fresh")
    );
    idle(&mut reader).await;
    assert_eq!(mind.asked(), 3);
    for (index, raw) in mind.asks().iter().enumerate() {
        let request: serde_json::Value = serde_json::from_str(raw).expect("actual request");
        assert_eq!(
            request["model"],
            if index == 0 { "fake/one" } else { "fake/two" }
        );
        let context = request["context"].to_string();
        assert!(context.contains("FROZEN-BEFORE-SWITCH"));
        assert!(!context.contains("cancelled-generation"));
        assert_eq!(context.contains("NEW-AFTER-SWITCH"), index == 2);
    }
    let (_, _, snapshot) = fixture.attach().await;
    let workflows: Vec<_> = snapshot
        .iter()
        .filter(|entry| matches!(entry, Entry::Workflow { .. }))
        .collect();
    assert_eq!(workflows.len(), 2);
    assert_eq!(workflows.first().copied(), Some(&frozen));
    assert_eq!(fixture.replay().await, snapshot);
    let preferences: serde_json::Value = serde_json::from_slice(
        &std::fs::read("/tmp/data/magi/chosen.json").expect("private model preference"),
    )
    .expect("preferences");
    assert_eq!(
        preferences
            .as_object()
            .expect("directory preferences")
            .len(),
        1
    );
    assert_eq!(
        preferences[fixture.dir.to_str().expect("cwd")]["model"],
        "fake/two"
    );
}
