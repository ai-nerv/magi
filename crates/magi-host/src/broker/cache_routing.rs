use super::*;
use magi_testkit::{
    Mind,
    mind::{stop_line, text_line},
};

async fn request(
    mind: &Mind,
    context: &Context,
    wants: &Wants,
    quiet: std::time::Duration,
) -> Result<(), Trouble> {
    asking(
        Asked {
            program: mind.program().to_str().expect("program"),
            environ: &Default::default(),
            model: "fake/cache-routing",
            context,
            wants,
        },
        |_| {},
        |_| {},
        quiet,
    )
    .await
}

#[tokio::test]
async fn transport_eof_releases_the_actual_preferred_upstream() {
    let served = r#"{"event":"served","provider":"sticky"}"#;
    let mind = Mind::turns(
        "cache-routing-eof",
        &[
            &[served, &stop_line()],
            &[&text_line("partial without terminal")],
            &[&stop_line()],
        ],
    );
    let context = Context {
        system: Some("cache routing eof".into()),
        ..Default::default()
    };
    let wants = Wants::default();
    let quiet = std::time::Duration::from_secs(2);
    request(&mind, &context, &wants, quiet)
        .await
        .expect("served");
    assert_eq!(
        request(&mind, &context, &wants, quiet)
            .await
            .expect_err("eof")
            .why,
        Refusal::Transport
    );
    request(&mind, &context, &wants, quiet)
        .await
        .expect("next request");
    let asks = mind
        .asks()
        .iter()
        .map(|ask| serde_json::from_str::<Ask>(ask).expect("ask"))
        .collect::<Vec<_>>();
    assert_eq!(asks.len(), 3);
    assert_eq!(asks[0].wants.provider, None);
    assert_eq!(asks[1].wants.provider.as_deref(), Some("sticky"));
    assert_eq!(asks[2].wants.provider, None);
}

#[tokio::test]
async fn quiet_transport_releases_but_explicit_preference_still_wins() {
    let served = Mind::saying(
        "cache-routing-served",
        &[r#"{"event":"served","provider":"sticky"}"#, &stop_line()],
    );
    let silent = Mind::silent("cache-routing-quiet");
    let context = Context {
        system: Some("cache routing quiet".into()),
        ..Default::default()
    };
    let wants = Wants::default();
    request(&served, &context, &wants, std::time::Duration::from_secs(2))
        .await
        .expect("served");
    let chosen = Wants {
        provider: Some("explicit".into()),
        ..Default::default()
    };
    request(
        &served,
        &context,
        &chosen,
        std::time::Duration::from_secs(2),
    )
    .await
    .expect("explicit");
    let requests = served.asks();
    let explicit: Ask = serde_json::from_str(&requests[1]).expect("explicit ask");
    assert_eq!(explicit.wants.provider.as_deref(), Some("explicit"));
    let helper = Context {
        system: Some("independent helper stream".into()),
        ..Default::default()
    };
    request(&served, &helper, &wants, std::time::Duration::from_secs(2))
        .await
        .expect("helper served");
    assert_eq!(
        request(
            &silent,
            &context,
            &wants,
            std::time::Duration::from_millis(50)
        )
        .await
        .expect_err("quiet")
        .why,
        Refusal::Transport
    );
    assert_eq!(
        sticky(&stream_of("fake/cache-routing", &context), &wants).provider,
        None
    );
    assert_eq!(
        sticky(&stream_of("fake/cache-routing", &helper), &wants)
            .provider
            .as_deref(),
        Some("sticky")
    );
    let_go(&stream_of("fake/cache-routing", &helper));
}

#[tokio::test]
async fn spawn_failure_also_releases_a_remembered_upstream() {
    let mind = Mind::saying(
        "cache-routing-spawn",
        &[r#"{"event":"served","provider":"sticky"}"#, &stop_line()],
    );
    let context = Context {
        system: Some("cache routing spawn".into()),
        ..Default::default()
    };
    let wants = Wants::default();
    request(&mind, &context, &wants, std::time::Duration::from_secs(2))
        .await
        .expect("served");
    let missing = mind.program().with_file_name("missing-program");
    let why = asking(
        Asked {
            program: missing.to_str().expect("path"),
            environ: &Default::default(),
            model: "fake/cache-routing",
            context: &context,
            wants: &wants,
        },
        |_| {},
        |_| {},
        std::time::Duration::from_secs(2),
    )
    .await
    .expect_err("missing program");
    assert_eq!(why.why, Refusal::Transport);
    assert_eq!(
        sticky(&stream_of("fake/cache-routing", &context), &wants).provider,
        None
    );
}
