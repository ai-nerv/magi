#![allow(clippy::expect_used)]

use magi_host::{catalog::Catalog, scribe::Scribe, session::Session};
use magi_ipc::{FrameReader, FrameWriter, family::Family};
use magi_proto::{AgentStatus, Entry, HarnessEvent, SessionId, UiCommand};
use magi_testkit::{Scratch, memory::Serving};
use magi_tools::supplier::Tooling;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

#[cfg(test)]
#[path = "workflow_submission/lifecycle.rs"]
mod lifecycle;

#[cfg(test)]
#[path = "workflow_submission/worktrees.rs"]
mod worktrees;

#[cfg(test)]
#[path = "workflow_submission/authority.rs"]
mod authority;

#[cfg(test)]
#[path = "workflow_submission/models.rs"]
mod models;

#[cfg(test)]
#[path = "workflow_submission/observing.rs"]
mod observing;

fn selected(name: &str) -> Option<PathBuf> {
    let Some(path) = std::env::var_os(name).map(PathBuf::from) else {
        magi_testkit::live::unavailable(&format!("{name} required for workflow submission"));
        return None;
    };
    assert!(path.is_absolute() && path.is_file());
    Some(path)
}

fn utility(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|root| root.join(name))
        .find(|path| path.is_file())
        .expect("fixture utility")
}

async fn event(reader: &mut Reader, matches: impl Fn(&HarnessEvent) -> bool) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = reader.read::<HarnessEvent>().await.expect("host event");
            if matches(&event) {
                return event;
            }
        }
    })
    .await
    .expect("event barrier")
}

async fn prompt(writer: &mut Writer, text: &str) {
    writer
        .write(&UiCommand::SubmitPrompt {
            text: text.into(),
            aside: "private aside".into(),
            images: Vec::new(),
        })
        .await
        .expect("submit");
}

struct Fixture {
    dir: Scratch,
    memory: Serving,
    host: tokio::task::JoinHandle<Result<(), magi_host::HostError>>,
    id: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.host.abort();
    }
}

impl Fixture {
    async fn start(name: &str, stalled: bool) -> Option<Self> {
        Self::configured(name, stalled, |_| {}).await
    }

    async fn configured(
        name: &str,
        stalled: bool,
        configure: impl FnOnce(&mut Catalog),
    ) -> Option<Self> {
        let binary = selected("BALTHASAR_TEST_BINARY")?;
        let casper = selected("CASPER_TEST_BINARY")?;
        let dir = Scratch::new("workflow-submit", name);
        let memory = Serving::start_program(&dir, name, &binary).await?;
        let mut catalog = Catalog::empty();
        catalog.cwd = dir.to_path_buf();
        for (key, part) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "run"),
        ] {
            catalog
                .environ
                .insert(key.into(), dir.join(part).display().to_string());
        }
        catalog
            .environ
            .insert("PATH".into(), dir.join("empty-bin").display().to_string());
        catalog.mind = dir.join("no-model-provider").display().to_string();
        let program = if stalled {
            let program = dir.join("stalled-helper");
            let script = format!(
                "#!{}\nprintf '%s' \"$$\" > \"$WORKFLOW_MARKER\"\nexec {} 30\n",
                utility("sh").display(),
                magi_proto::permit::quoted(utility("sleep").to_str().expect("sleep path")),
            );
            std::fs::write(&program, script).expect("helper script");
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o500))
                .expect("executable helper");
            catalog.environ.insert(
                "WORKFLOW_MARKER".into(),
                dir.join("pid").display().to_string(),
            );
            program
        } else {
            casper
        };
        catalog.tooling = Tooling::of(program.to_str().expect("helper path"));
        catalog.tooling.pin = Some(format!(
            "{:x}",
            Sha256::digest(std::fs::read(&program).expect("helper bytes"))
        ));
        configure(&mut catalog);
        let backend = catalog
            .chosen
            .as_ref()
            .map(|name| catalog.backend(name).expect("fixture model"));
        let path = dir.join("host.sock");
        let host = tokio::spawn(magi_host::serve_on(
            magi_ipc::bind(&path).await.expect("bind"),
            Session::recorded(SessionId::new(name), Vec::new()),
            backend,
            catalog,
            Some(memory.socket().to_owned()),
        ));
        Some(Self {
            dir,
            memory,
            host,
            id: name.into(),
        })
    }

    async fn attach(&self) -> (Reader, Writer, Vec<Entry>) {
        let path = self.dir.join("host.sock");
        let (read, write) = magi_ipc::connect(&path)
            .await
            .expect("connect")
            .into_split();
        let mut writer = FrameWriter::new(write);
        writer
            .write(&UiCommand::AttachLatest {
                draws: false,
                key: magi_ipc::read_key(&path),
            })
            .await
            .expect("attach");
        let mut reader = FrameReader::new(read);
        let HarnessEvent::SessionSnapshot { entries, .. } = event(&mut reader, |e| {
            matches!(e, HarnessEvent::SessionSnapshot { .. })
        })
        .await
        else {
            unreachable!()
        };
        (reader, writer, entries)
    }

    async fn replay(&self) -> Vec<Entry> {
        self.replay_of(&self.id).await
    }

    async fn replay_of(&self, id: &str) -> Vec<Entry> {
        let family = Family::dial(self.memory.socket())
            .await
            .expect("dial history");
        let mut scribe = Scribe::over(
            family,
            Some(self.memory.socket().to_owned()),
            &SessionId::new(id),
        );
        scribe
            .replay()
            .await
            .expect("replay")
            .into_iter()
            .map(|(_, entry)| entry)
            .collect()
    }
}

fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("source directory");
    std::fs::write(path, text).expect("source file");
}

#[tokio::test]
async fn socket_submission_uses_native_reader_and_replays_frozen_workflow_in_fifo_order() {
    let Some(fixture) = Fixture::start("native", false).await else {
        return;
    };
    let root = fixture.dir.join(".magi/skills");
    let source = "---\nname: review\ndescription: Native review\n---\nLiteral $SECRET `command`\n";
    put(&root.join("review/SKILL.md"), source);
    put(
        &fixture.dir.join(".agents/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Shadow\n---\nnot selected\n",
    );
    let (mut reader, mut writer, _) = fixture.attach().await;
    let arguments = "literal $(touch forbidden)\n\"quoted\"";
    let prompt_text = format!("/skill review {arguments}");
    prompt(&mut writer, &prompt_text).await;
    prompt(&mut writer, "ordinary after workflow").await;
    let mut seen = Vec::new();
    let mut warned = false;
    let mut finished = false;
    tokio::time::timeout(Duration::from_secs(20), async {
        while seen.len() < 2 || !warned || !finished {
            match reader.read::<HarnessEvent>().await.expect("event") {
                HarnessEvent::WorkflowMessage { text, workflow, .. } => {
                    assert_eq!(text, prompt_text);
                    assert_eq!(workflow.body, "Literal $SECRET `command`\n");
                    assert_eq!(workflow.arguments, arguments);
                    assert_eq!(workflow.root, root.to_str().expect("root"));
                    assert_eq!(workflow.digest, format!("{:x}", Sha256::digest(source)));
                    seen.push("workflow");
                    std::fs::remove_dir_all(&root).expect("remove selected source");
                }
                HarnessEvent::UserMessage { text, .. } if text == "ordinary after workflow" => {
                    seen.push("ordinary")
                }
                HarnessEvent::Noticed { text, .. } if text.contains("duplicate") => warned = true,
                HarnessEvent::StatusChanged {
                    status: AgentStatus::Idle,
                    ..
                } if seen.len() == 2 => finished = true,
                HarnessEvent::Refused { message, .. } => panic!("unexpected refusal: {message}"),
                _ => {}
            }
        }
    })
    .await
    .expect("workflow, ordinary and duplicate events");
    assert_eq!(seen, ["workflow", "ordinary"]);
    let (_, _, entries) = fixture.attach().await;
    assert_eq!(entries.len(), 4);
    assert_eq!(fixture.replay().await, entries);
    let workflow = entries
        .iter()
        .find(|e| matches!(e, Entry::Workflow { .. }))
        .expect("workflow");
    assert!(
        matches!(workflow, Entry::Workflow { text, aside, workflow, .. }
        if text == &prompt_text && aside == "private aside" && workflow.arguments == arguments && workflow.body == "Literal $SECRET `command`\n")
    );
    assert!(!fixture.dir.join("forbidden").exists());
}

async fn started(path: &Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("helper started")
}

async fn reaped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled helper reaped before its own timeout");
}

async fn cancellation(name: &str, command: UiCommand, other: bool, detach: bool) {
    let Some(fixture) = Fixture::start(name, true).await else {
        return;
    };
    let (mut reader, mut writer, _) = fixture.attach().await;
    let (mut second, mut controller, _) = fixture.attach().await;
    prompt(&mut writer, "/skill review pending").await;
    let pid = started(&fixture.dir.join("pid")).await;
    if other {
        controller
            .write(&command)
            .await
            .expect("other controller command");
    } else {
        writer.write(&command).await.expect("control command");
    }
    reaped(pid).await;
    if !detach {
        event(&mut reader, |e| matches!(e, HarnessEvent::Refused { message, .. } if message.contains("preparation"))).await;
    }
    let (_, _, entries) = fixture.attach().await;
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, Entry::Workflow { .. } | Entry::User { .. }))
    );
    prompt(&mut controller, "ordinary still accepted").await;
    event(&mut second, |e| matches!(e, HarnessEvent::UserMessage { text, .. } if text == "ordinary still accepted")).await;
    event(&mut second, |e| {
        matches!(
            e,
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
    assert!(
        !fixture
            .replay()
            .await
            .iter()
            .any(|e| matches!(e, Entry::Workflow { .. }))
    );
}

#[tokio::test]
async fn interrupt_remains_responsive_during_acquisition() {
    cancellation("interrupt", UiCommand::Interrupt, false, false).await;
}

#[tokio::test]
async fn another_controller_can_cancel_acquisition() {
    cancellation("other", UiCommand::Interrupt, true, false).await;
}

#[tokio::test]
async fn detach_cancels_only_unaccepted_owned_preparation() {
    cancellation("detach", UiCommand::Detach, false, true).await;
}

#[tokio::test]
async fn model_change_invalidates_inflight_preparation_even_if_refused() {
    cancellation(
        "model",
        UiCommand::SetModel {
            name: "missing/model".into(),
        },
        true,
        false,
    )
    .await;
}

#[tokio::test]
async fn resume_invalidates_inflight_preparation_even_if_target_is_missing() {
    cancellation(
        "resume",
        UiCommand::Resume {
            id: "missing-session".into(),
        },
        true,
        false,
    )
    .await;
}

#[tokio::test]
async fn branch_invalidates_inflight_preparation() {
    cancellation("branch", UiCommand::Branch { keeps: Some(0) }, true, false).await;
}

#[tokio::test]
async fn catalog_listing_is_lazy_and_contains_metadata_errors_without_journalling() {
    let Some(fixture) = Fixture::start("catalog", false).await else {
        return;
    };
    let root = fixture.dir.join(".magi/skills");
    put(
        &root.join("review/SKILL.md"),
        "---\nname: review\ndescription: Révision 日本語\n---\nPRIVATE_BODY\n",
    );
    put(&root.join("broken/SKILL.md"), "invalid metadata");
    put(
        &root.join("oversized/SKILL.md"),
        &format!(
            "---\nname: oversized\ndescription: Too large\n---\n{}",
            "x".repeat(65536)
        ),
    );
    let mut lazy = b"---\nname: lazy\ndescription: Lazy\n---\n".to_vec();
    lazy.extend(vec![0xff; 60 * 1024]);
    std::fs::create_dir_all(root.join("lazy")).expect("lazy directory");
    std::fs::write(root.join("lazy/SKILL.md"), lazy).expect("lazy invalid body");
    let (mut reader, mut writer, _) = fixture.attach().await;
    writer
        .write(&UiCommand::RefreshWorkflows)
        .await
        .expect("refresh");
    let listed = event(&mut reader, |e| {
        matches!(e, HarnessEvent::WorkflowsListed { .. })
    })
    .await;
    let HarnessEvent::WorkflowsListed { entries, error, .. } = &listed else {
        unreachable!()
    };
    assert!(error.is_none(), "{error:?}");
    assert_eq!(entries.len(), 4);
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "oversized" && entry.error.is_some())
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "broken" && entry.error.is_some())
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "lazy" && entry.error.is_none())
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "review" && entry.description == "Révision 日本語")
    );
    for encoding in [magi_ipc::Wire::Json, magi_ipc::Wire::Cbor] {
        let bytes = encoding.write(&listed).expect("encode");
        assert_eq!(
            magi_ipc::Wire::read::<HarnessEvent>(&bytes).expect("decode"),
            listed
        );
        let bytes = encoding
            .write(&UiCommand::RefreshWorkflows)
            .expect("command encode");
        assert_eq!(
            magi_ipc::Wire::read::<UiCommand>(&bytes).expect("command decode"),
            UiCommand::RefreshWorkflows
        );
    }
    assert!(
        !serde_json::to_string(&listed)
            .expect("json")
            .contains("PRIVATE_BODY")
    );
    let (_, _, entries) = fixture.attach().await;
    assert!(entries.is_empty());
    assert!(fixture.replay().await.is_empty());
}

#[tokio::test]
async fn catalog_refresh_is_cancelled_without_blocking_interrupt() {
    let Some(fixture) = Fixture::start("catalog-cancel", true).await else {
        return;
    };
    let (mut reader, mut writer, _) = fixture.attach().await;
    writer
        .write(&UiCommand::RefreshWorkflows)
        .await
        .expect("refresh");
    let pid = started(&fixture.dir.join("pid")).await;
    writer
        .write(&UiCommand::Interrupt)
        .await
        .expect("interrupt");
    reaped(pid).await;
    let listed = event(&mut reader, |e| {
        matches!(e, HarnessEvent::WorkflowsListed { .. })
    })
    .await;
    assert!(
        matches!(listed, HarnessEvent::WorkflowsListed { entries, error: Some(_), .. } if entries.is_empty())
    );
    assert!(fixture.replay().await.is_empty());
}

#[tokio::test]
async fn explicit_workspace_selects_only_its_project_roots_and_preserves_user_root() {
    for absolute in [false, true] {
        let name = if absolute { "ws-abs" } else { "ws-rel" };
        let Some(fixture) = Fixture::configured(name, false, |catalog| {
            let workspace = catalog.cwd.join("worktree");
            catalog.cwd = catalog.cwd.join("nested");
            catalog.workflow_workspace = Some(if absolute {
                workspace
            } else {
                "selected".into()
            });
        })
        .await
        else {
            return;
        };
        let workspace = fixture.dir.join(if absolute {
            "worktree"
        } else {
            "nested/selected"
        });
        let root = workspace.join(".magi/skills");
        let source = "---\nname: review\ndescription: Selected workspace\n---\nselected body\n";
        put(&root.join("review/SKILL.md"), source);
        if absolute {
            put(&workspace.join(".git"), "gitdir: /not-followed\n");
        }
        for path in [
            ".magi/skills/ancestor/SKILL.md",
            "nested/.magi/skills/launch/SKILL.md",
        ] {
            put(&fixture.dir.join(path), "must not be discovered");
        }
        put(
            &workspace.join(".agents/skills/review/SKILL.md"),
            "---\nname: review\ndescription: Shadow\n---\nshadow body\n",
        );
        put(
            &fixture.dir.join("config/magi/skills/personal/SKILL.md"),
            "---\nname: personal\ndescription: User workflow\n---\nuser body\n",
        );
        let (mut reader, mut writer, _) = fixture.attach().await;
        writer
            .write(&UiCommand::RefreshWorkflows)
            .await
            .expect("refresh");
        let listed = event(&mut reader, |e| {
            matches!(e, HarnessEvent::WorkflowsListed { .. })
        })
        .await;
        let HarnessEvent::WorkflowsListed {
            entries,
            warnings,
            error,
        } = listed
        else {
            unreachable!()
        };
        assert!(error.is_none(), "{error:?}");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["personal", "review"]
        );
        assert!(entries.iter().all(|entry| entry.error.is_none()));
        assert_eq!(entries[0].source, magi_proto::workflow::SourceClass::User);
        assert_eq!(
            entries[1].source,
            magi_proto::workflow::SourceClass::Project
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("duplicate"));
        assert!(fixture.replay().await.is_empty());
        prompt(&mut writer, "/skill review literal").await;
        let selected = event(&mut reader, |e| {
            matches!(e, HarnessEvent::WorkflowMessage { .. })
        })
        .await;
        let HarnessEvent::WorkflowMessage { workflow, .. } = selected else {
            unreachable!()
        };
        assert_eq!(workflow.root, root.to_str().expect("root"));
        assert_eq!(workflow.body, "selected body\n");
        assert_eq!(workflow.arguments, "literal");
        assert_eq!(workflow.digest, format!("{:x}", Sha256::digest(source)));
        event(&mut reader, |e| {
            matches!(
                e,
                HarnessEvent::StatusChanged {
                    status: AgentStatus::Idle,
                    ..
                }
            )
        })
        .await;
        std::fs::remove_dir_all(&workspace).expect("remove workspace");
        let (_, _, snapshot) = fixture.attach().await;
        assert_eq!(fixture.replay().await, snapshot);
        assert!(
            snapshot
                .iter()
                .any(|entry| matches!(entry, Entry::Workflow { .. }))
        );
    }
}

#[tokio::test]
async fn unsafe_explicit_workspace_refuses_without_falling_back_to_launch() {
    let Some(fixture) = Fixture::configured("ws-link", false, |catalog| {
        catalog.workflow_workspace = Some("selected".into());
    })
    .await
    else {
        return;
    };
    let source = "---\nname: review\ndescription: Must not load\n---\nnot admitted\n";
    put(&fixture.dir.join(".magi/skills/review/SKILL.md"), source);
    put(
        &fixture.dir.join("target/.magi/skills/review/SKILL.md"),
        source,
    );
    std::os::unix::fs::symlink(fixture.dir.join("target"), fixture.dir.join("selected"))
        .expect("workspace symlink");
    let (mut reader, mut writer, _) = fixture.attach().await;
    writer
        .write(&UiCommand::RefreshWorkflows)
        .await
        .expect("refresh");
    let listed = event(&mut reader, |e| {
        matches!(e, HarnessEvent::WorkflowsListed { .. })
    })
    .await;
    assert!(
        matches!(listed, HarnessEvent::WorkflowsListed { entries, error: Some(_), .. } if entries.is_empty())
    );
    prompt(&mut writer, "/skill review").await;
    event(&mut reader, |e| matches!(e, HarnessEvent::Refused { .. })).await;
    let (_, _, snapshot) = fixture.attach().await;
    assert!(
        snapshot
            .iter()
            .all(|entry| !matches!(entry, Entry::Workflow { .. }))
    );
    assert!(
        fixture
            .replay()
            .await
            .iter()
            .all(|entry| !matches!(entry, Entry::Workflow { .. }))
    );
}
