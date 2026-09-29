use super::*;
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, HarnessEvent, ModelInfo, ask::Card};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn card(name: &str) -> Card {
    serde_json::from_value(json!({"id":format!("ollama/{name}"),"provider":"ollama",
        "name":name,"api":"openai-completions","ready":true,"reasons":false,
        "context_window":32000,"max_output":8000}))
    .expect("refresh fixture")
}

fn fixture() -> (Scratch, Models) {
    let dir = Scratch::new("magi", "refresh");
    let program = dir.join("mind");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\n[ \"$*\" = \"models --json --refresh\" ] || exit 1\nexec /bin/cat '{}'\n",
            dir.join("reply").display()
        ),
    )
    .expect("refresh fixture");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
        .expect("refresh fixture");
    let mut catalog = crate::catalog::Catalog::empty();
    catalog.mind = program.to_string_lossy().into_owned();
    catalog.cards = vec![card("old")];
    (dir, Models::new(catalog))
}

#[tokio::test]
async fn refresh_updates_selection_and_snapshot_without_switching_active_model() {
    let (dir, models) = fixture();
    let session = Arc::new(Mutex::new(crate::open_session(1, "")));
    session.lock().await.set_model(Some(ModelInfo {
        name: "ollama/old".into(),
        context_window: 32000,
    }));
    let mut events = session.lock().await.subscribe();
    for cards in [vec![card("new")], vec![]] {
        std::fs::write(
            dir.join("reply"),
            json!({"ok":true,"refreshed":true,"failed":[],"result":cards}).to_string(),
        )
        .expect("refresh fixture");
        models.refresh(&session).await;
        let HarnessEvent::ModelsRefreshed { choices, warning } =
            events.recv().await.expect("refresh fixture")
        else {
            panic!("refresh event")
        };
        assert!(warning.is_none());
        assert_eq!(choices.len(), cards.len());
        assert_eq!(
            models.catalog.read().await.backend("ollama/new").is_some(),
            !cards.is_empty()
        );
        let HarnessEvent::SessionSnapshot {
            choices: snapshot,
            model,
            ..
        } = session.lock().await.snapshot(Cursor::ZERO)
        else {
            panic!("snapshot")
        };
        assert_eq!(snapshot, choices);
        assert_eq!(model.expect("refresh fixture").name, "ollama/old");
    }
}

#[tokio::test]
async fn refresh_failure_and_unsupported_provider_preserve_previous_choices() {
    let (dir, models) = fixture();
    let session = Arc::new(Mutex::new(crate::open_session(1, "")));
    let mut events = session.lock().await.subscribe();
    for reply in [
        json!({"ok":true,"refreshed":true,"failed":["ollama"],"result":[]}),
        json!({"ok":true,"result":[]}),
        json!({"ok":false,"error":"refused"}),
        json!({"ok":true,"refreshed":true,"failed":[],"result":[{}]}),
    ] {
        std::fs::write(dir.join("reply"), reply.to_string()).expect("refresh fixture");
        models.refresh(&session).await;
        let HarnessEvent::ModelsRefreshed { choices, warning } =
            events.recv().await.expect("refresh fixture")
        else {
            panic!("refresh event")
        };
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].name, "ollama/old");
        assert!(warning.is_some());
    }
}
