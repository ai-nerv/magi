use super::*;
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, HarnessEvent, ModelInfo, ask::Card};
use serde_json::json;

fn card(name: &str) -> Card {
    serde_json::from_value(json!({"id":format!("ollama/{name}"),"provider":"ollama",
        "name":name,"api":"openai-completions","ready":true,"reasons":false,"vision":true,
        "context_window":32000,"max_output":8000,"pricing":{"input":"0.3","output":"1.2"},"popularity_rank":7}))
    .expect("refresh fixture")
}

fn fixture() -> (Scratch, Models) {
    let dir = Scratch::new("magi", "refresh");
    let program = dir.join("mind");
    let saved = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "umask 077; printf '%s' \"$2\" > \"$1\" && chmod 700 \"$1\"",
            "refresh-fixture",
        ])
        .arg(&program)
        .arg(format!(
            "#!/bin/sh\n[ \"$*\" = \"models --json --refresh\" ] || exit 1\nexec /bin/cat '{}'\n",
            dir.join("reply").display()
        ))
        .status()
        .expect("refresh fixture");
    assert!(saved.success(), "refresh fixture writer failed");
    while let Err(why) = std::process::Command::new(&program).status() {
        assert_eq!(why.raw_os_error(), Some(26), "{why}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(choices.len(), cards.len());
        if let Some(choice) = choices.first() {
            assert!(choice.vision);
            assert_eq!(
                choice.pricing.as_ref().expect("prices").input.as_deref(),
                Some("0.3")
            );
            assert_eq!(choice.popularity_rank, Some(7));
        }
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
