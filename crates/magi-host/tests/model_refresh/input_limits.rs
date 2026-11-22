use super::*;
use serde_json::Value;

fn limits(edge: u64) -> Value {
    json!({"max_request_bytes":33554432,"images":{"max_per_message":20,"max_per_request":100,
        "resize":{"max_width":edge,"max_height":edge,"max_bytes":4718592,"jpeg_quality":80}}})
}

fn card(edge: u64) -> Value {
    json!({"id":"fixture/m","provider":"fixture","name":"M","api":"anthropic-messages",
        "ready":true,"vision":true,"capabilities":{"input":["text","image"],"input_limits":limits(edge)}})
}

fn legacy() -> Value {
    json!({"id":"fixture/legacy","provider":"fixture","name":"Legacy","api":"openai-completions","ready":true})
}

fn found(choice: &magi_proto::ModelChoice) -> Value {
    serde_json::to_value(
        choice
            .capabilities
            .as_ref()
            .expect("capabilities")
            .input_limits
            .as_ref()
            .expect("limits"),
    )
    .expect("limits")
}

fn selected(snapshot: HarnessEvent) -> magi_proto::ModelChoice {
    let HarnessEvent::SessionSnapshot {
        model: Some(model),
        choices,
        ..
    } = snapshot
    else {
        panic!("selected model snapshot")
    };
    choices
        .into_iter()
        .find(|choice| choice.name == model.name)
        .expect("selected model metadata")
}

async fn refreshed(
    reader: &mut FrameReader<OwnedReadHalf>,
) -> (Vec<magi_proto::ModelChoice>, Option<String>) {
    loop {
        if let HarnessEvent::ModelsRefreshed { choices, warning } = next(reader).await {
            return (choices, warning);
        }
    }
}

#[tokio::test]
async fn input_limits_reach_refresh_selection_and_snapshots_without_partial_invalid_updates() {
    if thinking::isolated(
        "input_limits::input_limits_reach_refresh_selection_and_snapshots_without_partial_invalid_updates",
    ) {
        return;
    }
    let dir = Scratch::new("magi-input-limits", "session");
    let program = dir.join("mind");
    let reply = dir.join("models.json");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\n[ \"$*\" = \"models --json --refresh\" ] || exit 1\nexec cat '{}'\n",
            reply.display()
        ),
    )
    .expect("program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let mut catalog = magi_host::catalog::Catalog::empty();
    catalog.mind = program.display().to_string();
    catalog.memory = "/fixture-no-real-memory".into();
    catalog.tooling.program = "/fixture-no-real-tools".into();
    catalog.cwd = dir.to_path_buf();
    catalog.cards = [card(2000), legacy()]
        .into_iter()
        .map(|raw| serde_json::from_value(raw).expect("card"))
        .collect();
    let backend = catalog.backend("fixture/m").expect("backend");
    let path = dir.join("s.sock");
    let task = tokio::spawn(magi_host::serve_catalog(
        magi_ipc::bind(&path).await.expect("listener"),
        magi_host::open_session(1, "limits"),
        Some(backend),
        catalog,
    ));
    let (mut first, mut writer, snapshot) = attached(&path).await;
    let (mut second, _other) = attach(&path).await;
    assert_eq!(found(&selected(snapshot)), limits(2000));
    for (edge, accepted) in [(1200, true), (0, false)] {
        std::fs::write(
            &reply,
            serde_json::to_vec(
                &json!({"ok":true,"refreshed":true,"failed":[],"result":[card(edge),legacy()]}),
            )
            .expect("reply"),
        )
        .expect("model listing");
        writer
            .write(&UiCommand::RefreshModels)
            .await
            .expect("refresh");
        for reader in [&mut first, &mut second] {
            let (choices, warning) = refreshed(reader).await;
            assert_eq!(
                found(
                    choices
                        .iter()
                        .find(|choice| choice.name == "fixture/m")
                        .expect("model")
                ),
                limits(1200)
            );
            assert_eq!(warning.is_none(), accepted);
        }
        let (_, _, snapshot) = attached(&path).await;
        assert_eq!(found(&selected(snapshot)), limits(1200));
    }
    for name in ["fixture/legacy", "fixture/m"] {
        writer
            .write(&UiCommand::SetModel { name: name.into() })
            .await
            .expect("selection");
        for reader in [&mut first, &mut second] {
            loop {
                if let HarnessEvent::ModelChanged {
                    model: Some(model), ..
                } = next(reader).await
                {
                    assert_eq!(model.name, name);
                    break;
                }
            }
        }
        let (_, _, snapshot) = attached(&path).await;
        let choice = selected(snapshot);
        assert_eq!(choice.name, name);
        if name == "fixture/m" {
            assert_eq!(found(&choice), limits(1200));
        } else {
            assert!(choice.capabilities.is_none());
        }
    }
    task.abort();
    let _ = task.await;
}
