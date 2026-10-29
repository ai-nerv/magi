use super::{backend, session};
use magi_host::session::Session;
use magi_proto::{Entry, SessionId};
use magi_testkit::{
    Mind,
    mind::{call_lines, stop_line, text_line},
};
use magi_tools::{Cancel, Ops, Output, Registry, Tool};
use std::sync::Arc;
use tokio::sync::Mutex;

#[path = "files/fixture.rs"]
mod fixture;
use fixture::Provider;

fn mind(name: &str, calls: &[(&str, &str)]) -> Mind {
    let mut turns: Vec<Vec<String>> = calls
        .iter()
        .map(|(tool, args)| call_lines("reused-provider-id", tool, args).to_vec())
        .collect();
    turns.push(vec![text_line("finished"), stop_line()]);
    let rows: Vec<Vec<&str>> = turns
        .iter()
        .map(|turn| turn.iter().map(String::as_str).collect())
        .collect();
    Mind::turns(name, &rows.iter().map(Vec::as_slice).collect::<Vec<_>>())
}

async fn run(
    session: &Mutex<Session>,
    registry: &Registry,
    ops: &dyn Ops,
    calls: &[(&str, &str)],
    name: &str,
) -> Result<(), magi_host::HostError> {
    let mind = mind(name, calls);
    let scribe = Arc::new(Mutex::new(None));
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        magi_host::turn::run(session, &backend(&mind), registry, ops, &scribe),
    )
    .await
    .expect("bounded host turn")
}

enum Hook {
    Append,
    Cancel,
    FailAfterCompletion,
    CorruptCursor,
    Resume,
}

struct Observing {
    inner: Box<dyn Tool>,
    probe: Option<Box<dyn Tool>>,
    session: Arc<Mutex<Session>>,
    hook: Hook,
}

impl Tool for Observing {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }
    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops, cancel: &dyn Cancel) -> Output {
        if matches!(self.hook, Hook::Append) {
            self.session
                .try_lock()
                .expect("no session lock during tool")
                .commit(Entry::Notice {
                    text: "arrived during tool".into(),
                })
                .expect("later entry");
        }
        let mut output = self.inner.run(arguments, ops, cancel);
        assert!(
            output.file_delivery.is_some(),
            "fixture never reached native completion: {}",
            output.content
        );
        if let Some(probe) = &self.probe {
            let pending = probe.run(arguments, ops, cancel);
            assert!(
                pending.is_error && pending.content.contains("delivered observation"),
                "raw completion incorrectly granted an edit: {}",
                pending.content
            );
        }
        let mut session = self
            .session
            .try_lock()
            .expect("session available after completion");
        match self.hook {
            Hook::Append => {}
            Hook::Cancel => session.cancel().request(),
            Hook::FailAfterCompletion => {
                output.is_error = true;
                output.content = "injected final attachment failure".into();
            }
            Hook::CorruptCursor => {
                session
                    .amend(Entry::Notice {
                        text: "replaced pending call".into(),
                    })
                    .expect("replace test cursor");
            }
            Hook::Resume => {
                *session = Session::recorded(
                    SessionId::new("resumed-with-same-cursors"),
                    session.entries().to_vec(),
                );
            }
        }
        output
    }
}

fn hooked(provider: &Provider, session: &Arc<Mutex<Session>>, hook: Hook, probe: bool) -> Registry {
    let mut registry = provider.registry();
    let look = provider
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "look")
        .expect("native look");
    let change = if probe {
        Some(Box::new(
            provider
                .tools()
                .into_iter()
                .find(|tool| tool.name() == "change")
                .expect("native change"),
        ) as Box<dyn Tool>)
    } else {
        None
    };
    registry.register(Box::new(Observing {
        inner: Box::new(look),
        probe: change,
        session: session.clone(),
        hook,
    }));
    registry
}

#[tokio::test]
async fn checked_paths_and_exact_journal_delivery_control_guards_even_with_reused_provider_ids() {
    let provider = Provider::new("paired");
    let (session, _dir) = session("native-paired");
    let session = Arc::new(session);
    let registry = hooked(&provider, &session, Hook::Append, true);
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    let raw = "{\"path\":7,\"note\":\"raw\nnewline\"}";
    assert!(serde_json::from_str::<serde_json::Value>(raw).is_err());
    run(
        &session,
        &registry,
        &ops,
        &[
            ("look", raw),
            ("change", r#"{"path":"7"}"#),
            ("change", r#"{"path":"7"}"#),
        ],
        "paired-native",
    )
    .await
    .expect("turn");
    let calls = provider.calls();
    assert_eq!(calls.len(), 3);
    for call in &calls {
        assert_eq!(call["args"]["path"], "7");
        assert!(call["args"].get("revision_contract").is_none());
    }
    assert_eq!(calls[0]["args"]["note"], "raw\nnewline");
    assert_eq!(calls[1]["expected_revision"], format!("cfr1:{:064}", 1));
    assert_eq!(calls[2]["expected_revision"], format!("cfr1:{:064}", 2));
    assert_ne!(calls[1]["mutation_id"], calls[2]["mutation_id"]);
    assert!(
        calls[1]["mutation_id"]
            .as_str()
            .expect("logical identity")
            .len()
            <= 128
    );
    let held = session.lock().await;
    let tools: Vec<_> = held
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool {
                id, args, result, ..
            } => Some((id, args, result.as_ref().expect("paired result"))),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0].1, raw);
    assert!(
        tools
            .iter()
            .all(|(id, _, result)| id.as_str() == "reused-provider-id" && !result.is_error)
    );
    assert_eq!(tools[0].2.output, "observed");
    assert!(tools[1].2.output.ends_with(&format!("{:064}", 2)));
    assert!(tools[2].2.output.ends_with(&format!("{:064}", 3)));
    assert!(
        held.entries()
            .iter()
            .any(|entry| matches!(entry,Entry::Notice {text} if text=="arrived during tool"))
    );
}

#[tokio::test]
async fn ordinary_and_failed_results_and_other_agent_reads_never_grant_parent_mutation_receipts() {
    let provider = Provider::new("ownership");
    let registry = provider.registry();
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    let (child, _child_dir) = session("native-child");
    run(
        &child,
        &registry,
        &ops,
        &[("look", r#"{"path":"source"}"#)],
        "child-observes",
    )
    .await
    .expect("child");
    let (parent, _parent_dir) = session("native-parent");
    run(
        &parent,
        &registry,
        &ops,
        &[
            ("change", r#"{"path":"source"}"#),
            ("nested", r#"{"path":"source"}"#),
            ("change", r#"{"path":"source"}"#),
            ("badread", r#"{"path":"source"}"#),
            ("change", r#"{"path":"source"}"#),
            ("look", r#"{"path":"source"}"#),
            ("change", r#"{"path":"source"}"#),
        ],
        "parent-must-reread",
    )
    .await
    .expect("parent");
    let calls = provider.calls();
    assert_eq!(
        calls.iter().filter(|call| call["tool"] == "change").count(),
        1
    );
    assert_eq!(calls.len(), 5);
    let held = parent.lock().await;
    let failed: Vec<_> = held
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool {
                name,
                result: Some(result),
                ..
            } if name == "change" => Some(result.is_error),
            _ => None,
        })
        .collect();
    assert_eq!(failed, vec![true, true, true, false]);
}

#[tokio::test]
async fn delivered_reread_repairs_a_suppressed_native_edit_in_the_same_batch() {
    let provider = Provider::new("progress-batch");
    let registry = provider.registry();
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    let (session, _dir) = session("native-progress-batch");
    let change = call_lines("change", "change", r#"{"path":"source"}"#);
    let look = call_lines("look", "look", r#"{"path":"source"}"#);
    let failed: Vec<_> = change.iter().map(String::as_str).collect();
    let repaired = [&look[0][..], &look[1], &change[0], &change[1], &change[2]];
    let mind = Mind::turns(
        "native-progress-batch",
        &[
            &failed,
            &failed,
            &failed,
            &repaired,
            &[&text_line("finished"), &stop_line()],
        ],
    );
    magi_host::turn::run(
        &session,
        &backend(&mind),
        &registry,
        &ops,
        &Arc::new(Mutex::new(None)),
    )
    .await
    .expect("native recovery turn");
    assert_eq!(mind.asked(), 5);
    let calls = provider.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["tool"], "look");
    assert_eq!(calls[1]["tool"], "change");
    let held = session.lock().await;
    let results: Vec<_> = held
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool { result, .. } => Some(result.as_ref().expect("paired result")),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 5);
    assert!(results[2].output.starts_with("Repeated-call guard:"));
    assert!(!results[3].is_error && !results[4].is_error);
}

#[tokio::test]
async fn cancelled_unjournalled_and_resumed_native_completions_cannot_refresh_delivered_state() {
    for (name, hook) in [
        ("cancelled", Hook::Cancel),
        ("failed-after-completion", Hook::FailAfterCompletion),
        ("wrong-cursor", Hook::CorruptCursor),
        ("resumed", Hook::Resume),
    ] {
        let provider = Provider::new(name);
        let (session, _dir) = session(name);
        let session = Arc::new(session);
        let registry = hooked(&provider, &session, hook, false);
        let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
        let result = run(
            &session,
            &registry,
            &ops,
            &[("look", r#"{"path":"source"}"#)],
            name,
        )
        .await;
        if name == "cancelled" || name == "failed-after-completion" {
            result.expect("cancelled read was journalled but not delivered to model");
            session.lock().await.cancel().clear();
        } else {
            let why = result.expect_err("late result must not amend").to_string();
            assert!(
                why.contains(if name == "resumed" {
                    "retired session"
                } else {
                    "identity"
                }),
                "{why}"
            );
        }
        run(
            &session,
            &registry,
            &ops,
            &[("change", r#"{"path":"source"}"#)],
            "reread-required",
        )
        .await
        .expect("guard refusal turn");
        assert_eq!(
            provider.calls().len(),
            1,
            "abandoned read enabled a mutation"
        );
        assert!(session.lock().await.entries().iter().any(|entry| matches!(entry,
            Entry::Tool {name,result:Some(result),..} if name=="change" && result.is_error && result.output.contains("reread"))));
    }
}

struct Denied(std::path::PathBuf);
impl Ops for Denied {
    fn cwd(&self) -> std::path::PathBuf {
        self.0.clone()
    }
    fn read(&self, _: &std::path::Path) -> Result<String, String> {
        Err("no direct reads".into())
    }
    fn write(&self, _: &std::path::Path, _: &str) -> Result<(), String> {
        Err("no direct writes".into())
    }
    fn allow(&self, _: &str, action: &magi_proto::permit::Action) -> Result<(), String> {
        assert!(
            matches!(action, magi_proto::permit::Action::Write { .. }),
            "native operation trusted a lying read permission verb"
        );
        Err("fixture denied native write".into())
    }
}

#[tokio::test]
async fn model_arguments_cannot_supply_guards_or_turn_an_unobserved_write_into_unchecked_overwrite()
{
    let provider = Provider::new("model-guards");
    let registry = provider.registry();
    let (session, _dir) = session("native-model-guards");
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    run(
        &session,
        &registry,
        &ops,
        &[("store", r#"{"path":"source"}"#)],
        "no-observation",
    )
    .await
    .expect("no-replace refusal");
    let first = provider.calls();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["expected_revision"], "absent");
    for field in [
        "file_revision",
        "file_recovery",
        "revision_contract",
        "expected_revision",
        "mutation_id",
    ] {
        let mut args = serde_json::json!({"path":"source"});
        args[field] = serde_json::Value::Null;
        run(
            &session,
            &registry,
            &ops,
            &[("store", &args.to_string())],
            "model-guard-refusal",
        )
        .await
        .expect("guard refused");
    }
    assert_eq!(
        provider.calls().len(),
        1,
        "model metadata reached native provider"
    );
    run(
        &session,
        &registry,
        &ops,
        &[
            ("look", r#"{"path":"source"}"#),
            ("store", r#"{"path":"source"}"#),
        ],
        "observed-write",
    )
    .await
    .expect("guarded overwrite");
    let calls = provider.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2]["expected_revision"], format!("cfr1:{:064}", 1));
    assert_ne!(calls[0]["mutation_id"], calls[2]["mutation_id"]);
    assert_eq!(
        std::fs::read_to_string(provider.dir.join("version")).expect("only guarded commit"),
        "2"
    );
    assert!(
        session
            .lock()
            .await
            .entries()
            .iter()
            .filter(|entry| matches!(entry,
        Entry::Tool {name,result:Some(result),..} if name=="store" && result.is_error))
            .count()
            >= 5
    );
}

#[tokio::test]
async fn lost_commit_replies_block_new_model_mutations_until_a_delivered_native_reread() {
    let provider = Provider::new("lost-commit");
    let registry = provider.registry();
    let (session, _dir) = session("native-lost-commit");
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    run(
        &session,
        &registry,
        &ops,
        &[
            ("look", r#"{"path":"source"}"#),
            ("lost", r#"{"path":"source"}"#),
            ("store", r#"{"path":"source"}"#),
        ],
        "unknown-is-not-create",
    )
    .await
    .expect("lost response handled");
    let calls = provider.calls();
    assert_eq!(
        calls.len(),
        2,
        "unknown mutation retried through a new model call"
    );
    assert_eq!(
        std::fs::read_to_string(provider.dir.join("version")).expect("actual fake commit"),
        "2"
    );
    assert!(session.lock().await.entries().iter().any(|entry| matches!(entry,
        Entry::Tool {name,result:Some(result),..} if name=="lost" && result.is_error && result.output.starts_with("Native file mutation outcome is unknown"))));
    run(
        &session,
        &registry,
        &ops,
        &[
            ("badread", r#"{"path":"source"}"#),
            ("store", r#"{"path":"source"}"#),
        ],
        "failed-read-is-not-recovery",
    )
    .await
    .expect("failed read handled");
    assert_eq!(provider.calls().len(), 3);
    run(
        &session,
        &registry,
        &ops,
        &[
            ("look", r#"{"path":"source"}"#),
            ("store", r#"{"path":"source"}"#),
        ],
        "delivered-recovery",
    )
    .await
    .expect("reread restores eligibility");
    let calls = provider.calls();
    assert_eq!(calls.len(), 5);
    assert_eq!(calls[4]["expected_revision"], format!("cfr1:{:064}", 2));
    assert_ne!(calls[1]["mutation_id"], calls[4]["mutation_id"]);
}

#[tokio::test]
async fn missing_target_recovery_needs_explicit_delivered_absence_and_old_providers_cannot_fake_support()
 {
    let provider = Provider::new("absent-recovery");
    let registry = provider.registry();
    let (session, _dir) = session("native-absent");
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    run(
        &session,
        &registry,
        &ops,
        &[
            ("lost_missing", r#"{"path":"missing"}"#),
            ("store", r#"{"path":"missing"}"#),
        ],
        "missing-uncertain",
    )
    .await
    .expect("unknown outcome");
    assert_eq!(provider.calls().len(), 1);
    std::fs::write(provider.dir.join("old-recovery-support"), "old").expect("old native peer");
    run(
        &session,
        &registry,
        &ops,
        &[
            ("look", r#"{"path":"missing"}"#),
            ("store", r#"{"path":"missing"}"#),
        ],
        "old-peer-no-proof",
    )
    .await
    .expect("old behavior");
    let calls = provider.calls();
    assert_eq!(calls.len(), 2, "failed old read resolved unknown creation");
    assert_eq!(calls[1]["file_recovery"], 1);
    std::fs::remove_file(provider.dir.join("old-recovery-support")).expect("supported peer");
    run(
        &session,
        &registry,
        &ops,
        &[
            ("look", r#"{"path":"missing"}"#),
            ("change", r#"{"path":"missing"}"#),
            ("store", r#"{"path":"missing"}"#),
        ],
        "successful-absence",
    )
    .await
    .expect("recovery");
    let calls = provider.calls();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[2]["file_recovery"], 1);
    assert_eq!(calls[3]["expected_revision"], "absent");
    assert!(calls[3]["args"].get("file_recovery").is_none());
    assert!(session.lock().await.entries().iter().any(|entry| matches!(entry,
        Entry::Tool {name,result:Some(result),..} if name=="change" && result.is_error && result.output.contains("delivered observation"))));
}

#[tokio::test]
async fn known_no_effect_refusals_are_not_misclassified_as_unknown_creation_outcomes() {
    let provider = Provider::new("known-refused");
    let registry = provider.registry();
    let original = std::fs::read(&provider.program).expect("role source");
    let (session, _dir) = session("native-known-refused");
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    std::fs::write(
        &provider.program,
        "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"file-admission\",\"revision_contract\":2}'\n",
    )
    .expect("old unsupported role");
    run(
        &session,
        &registry,
        &ops,
        &[("store", r#"{"path":"missing"}"#)],
        "known-unsent",
    )
    .await
    .expect("known refusal");
    assert!(provider.calls().is_empty());
    std::fs::write(&provider.program, original).expect("restored role");
    run(
        &session,
        &registry,
        &ops,
        &[
            ("reject", r#"{"path":"missing"}"#),
            ("store", r#"{"path":"missing"}"#),
        ],
        "known-typed-refusal",
    )
    .await
    .expect("no unresolved pin");
    let calls = provider.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["expected_revision"], "absent");
    assert_eq!(
        std::fs::read_to_string(provider.dir.join("created")).expect("created after safe refusal"),
        "created"
    );
}

#[tokio::test]
async fn permission_denial_precedes_native_admission_and_preserves_the_last_delivered_observation()
{
    let provider = Provider::new("denied");
    let registry = provider.registry();
    let (session, _dir) = session("native-denied");
    let ops = magi_tools::ops::Real::new(provider.dir.to_path_buf());
    run(
        &session,
        &registry,
        &ops,
        &[("look", r#"{"path":"source"}"#)],
        "observed-before-denial",
    )
    .await
    .expect("read");
    run(
        &session,
        &registry,
        &Denied(provider.dir.to_path_buf()),
        &[("change", r#"{"path":"source"}"#)],
        "denied-native",
    )
    .await
    .expect("denied");
    assert_eq!(provider.calls().len(), 1);
    run(
        &session,
        &registry,
        &ops,
        &[("change", r#"{"path":"source"}"#)],
        "allowed-native",
    )
    .await
    .expect("allowed");
    let calls = provider.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["expected_revision"], format!("cfr1:{:064}", 1));
}
