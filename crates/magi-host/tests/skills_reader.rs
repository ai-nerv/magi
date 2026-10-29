use magi_host::{
    cancel::Cancel,
    skills::{Catalog, Client, Diagnostic, Error, ReadFailure, Roots, SourceClass},
};
use magi_testkit::Scratch;
use magi_tools::supplier::Tooling;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[path = "skills_reader/permissions.rs"]
mod permissions;

fn selected() -> Option<PathBuf> {
    let Some(path) = std::env::var_os("CASPER_TEST_BINARY").map(PathBuf::from) else {
        magi_testkit::live::unavailable(
            "selected Casper is required for workflow reader interoperability",
        );
        return None;
    };
    assert!(
        path.is_absolute() && path.is_file(),
        "selected Casper must be an absolute existing file"
    );
    Some(path)
}

fn source(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n{body}")
}

#[allow(clippy::expect_used)]
fn put(root: &Path, name: &str, text: &str) {
    std::fs::create_dir_all(root.join(name)).expect("directory");
    std::fs::write(root.join(name).join("SKILL.md"), text).expect("skill");
}

#[allow(clippy::expect_used)]
fn client(fixture: &Scratch, binary: &Path, bwrap: bool) -> Client {
    let path = if bwrap {
        let backend = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
            .map(|path| path.join("bwrap"))
            .find(|path| path.is_file())
            .expect("bwrap prerequisite");
        let metadata = std::fs::metadata(&backend).expect("backend metadata");
        assert_eq!(
            metadata.permissions().mode() & 0o222,
            0,
            "backend fixture must be read-only"
        );
        assert_eq!(
            metadata.nlink(),
            1,
            "backend fixture must not have hardlink aliases"
        );
        backend.parent().expect("backend parent").to_owned()
    } else {
        fixture.join("empty-bin")
    };
    let mut environ = BTreeMap::new();
    for (key, value) in [
        ("HOME", fixture.join("home")),
        ("XDG_CONFIG_HOME", fixture.join("config")),
        ("XDG_DATA_HOME", fixture.join("data")),
        ("XDG_STATE_HOME", fixture.join("state")),
        ("XDG_CACHE_HOME", fixture.join("cache")),
        ("XDG_RUNTIME_DIR", fixture.join("run")),
        ("PATH", path),
        ("NERV_LOG", fixture.join("forbidden-log")),
        ("WORKFLOW_DEBUG_LOG", fixture.join("forbidden-debug-log")),
    ] {
        environ.insert(key.into(), value.to_str().expect("UTF8 fixture").into());
    }
    environ.insert(
        "CASPER_CONFIGURE".into(),
        "error('must not execute ambient config')".into(),
    );
    let mut tooling = Tooling::of(binary.to_str().expect("selected path"));
    tooling.pin = Some(format!(
        "{:x}",
        Sha256::digest(std::fs::read(binary).expect("binary bytes"))
    ));
    Client::new(tooling, environ)
}

#[allow(clippy::expect_used)]
async fn exercise(bwrap: bool) {
    let Some(binary) = selected() else {
        return;
    };
    let fixture = Scratch::new(
        "magi-workflows",
        if bwrap { "bubblewrap" } else { "landlock" },
    );
    let workspace = fixture.join("worktree/nested");
    let config = fixture.join("config");
    let roots = Roots::new(&workspace, None, &config).expect("roots");
    let project = &roots.sources()[0].root;
    let agents = &roots.sources()[1].root;
    let user = &roots.sources()[2].root;
    let text = source(
        "review",
        "Révision 日本語",
        "Literal $(touch forbidden); $SECRET `command`\n",
    );
    put(project, "review", &text);
    put(agents, "review", &source("review", "shadow", "agents body"));
    put(user, "review", &source("review", "shadow", "user body"));
    put(project, "bad", "not metadata");
    put(
        user,
        "bad",
        &source("bad", "shadow", "must not silently load"),
    );
    put(
        agents,
        "agent-only",
        &source("agent-only", "Agent", "agent body"),
    );
    put(user, "user-only", &source("user-only", "User", "user body"));
    put(project, "lazy", &source("lazy", "Lazy", &"x".repeat(65536)));
    let mut invalid_body = source("utf8-body", "Metadata only", "body").into_bytes();
    invalid_body.push(0xff);
    std::fs::create_dir_all(project.join("utf8-body")).expect("body fixture directory");
    std::fs::write(project.join("utf8-body/SKILL.md"), invalid_body).expect("invalid body fixture");
    std::fs::write(project.join("INVALID"), "ignored invalid name").expect("invalid name fixture");
    std::fs::write(
        config.join("magi/init.lua"),
        "error('not executable skill content')",
    )
    .expect("protected sibling config");
    let client = client(&fixture, &binary, bwrap);
    let cancel = Cancel::default();
    let catalog = Catalog::discover(&client, &roots, &cancel)
        .await
        .expect("real catalog");
    assert_eq!(
        catalog
            .entries()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        [
            "agent-only",
            "bad",
            "lazy",
            "review",
            "user-only",
            "utf8-body"
        ]
    );
    assert!(
        catalog
            .entries()
            .find(|entry| entry.name == "utf8-body")
            .expect("lazy entry")
            .metadata
            .is_ok()
    );
    assert_eq!(
        catalog.load(&client, "utf8-body", &cancel).await,
        Err(Error::Read(ReadFailure::Encoding))
    );
    assert_eq!(
        catalog
            .diagnostics()
            .iter()
            .filter(|d| matches!(d, Diagnostic::Duplicate { .. }))
            .count(),
        3
    );
    assert_eq!(
        catalog
            .diagnostics()
            .iter()
            .filter(|d| matches!(d, Diagnostic::InvalidNames { count: 1, .. }))
            .count(),
        1
    );
    assert!(matches!(
        catalog.load(&client, "bad", &cancel).await,
        Err(Error::Read(ReadFailure::Metadata))
    ));
    assert_eq!(
        catalog.load(&client, "lazy", &cancel).await,
        Err(Error::Read(ReadFailure::Oversized))
    );
    let loaded = catalog
        .load(&client, "review", &cancel)
        .await
        .expect("selected snapshot");
    assert_eq!(loaded.source.class, SourceClass::Project);
    assert_eq!(loaded.source.root, *project);
    assert_eq!(
        loaded.document.body,
        "Literal $(touch forbidden); $SECRET `command`\n"
    );
    assert_eq!(
        loaded.document.source_digest,
        format!("{:x}", Sha256::digest(text.as_bytes()))
    );
    assert_eq!(
        catalog
            .load(&client, "user-only", &cancel)
            .await
            .expect("user source")
            .source
            .class,
        SourceClass::User
    );
    put(
        project,
        "review",
        &source("review", "Révision 日本語", "new body"),
    );
    assert_eq!(
        catalog
            .load(&client, "review", &cancel)
            .await
            .expect("explicit new selection")
            .document
            .body,
        "new body"
    );
    assert_eq!(
        loaded.document.body,
        "Literal $(touch forbidden); $SECRET `command`\n"
    );
    put(
        project,
        "review",
        &source("review", "changed metadata", "new body"),
    );
    assert_eq!(
        catalog.load(&client, "review", &cancel).await,
        Err(Error::Changed)
    );
    std::fs::remove_file(project.join("review/SKILL.md")).expect("remove selected");
    assert_eq!(
        catalog.load(&client, "review", &cancel).await,
        Err(Error::Read(ReadFailure::NotFound))
    );
    assert!(!fixture.join("forbidden").exists());
    assert!(!fixture.join("forbidden-log").exists());
    assert!(!fixture.join("forbidden-debug-log").exists());
    assert_eq!(
        std::fs::read_to_string(config.join("magi/init.lua")).expect("config unchanged"),
        "error('not executable skill content')"
    );
    cancel.request();
    assert_eq!(
        Catalog::discover(&client, &roots, &cancel)
            .await
            .expect_err("cancelled")
            .cause,
        Error::Cancelled
    );
}

#[tokio::test]
async fn selected_casper_reads_precedence_and_snapshots_under_bubblewrap() {
    exercise(true).await;
}

#[tokio::test]
async fn selected_casper_reads_precedence_and_snapshots_under_landlock() {
    exercise(false).await;
}

#[tokio::test]
async fn missing_roots_are_empty_but_unsafe_roots_and_pin_failures_are_visible() {
    let Some(binary) = selected() else {
        return;
    };
    let fixture = Scratch::new("magi-workflows", "refusals");
    let roots = Roots::new(&fixture.join("work"), None, &fixture.join("config")).expect("roots");
    let client = client(&fixture, &binary, false);
    let catalog = Catalog::discover(&client, &roots, &Cancel::default())
        .await
        .expect("absent roots");
    assert_eq!(catalog.entries().count(), 0);
    let root = &roots.sources()[0].root;
    std::fs::create_dir_all(root.parent().expect("root parent")).expect("parent");
    std::fs::create_dir(fixture.join("target")).expect("target");
    std::os::unix::fs::symlink(fixture.join("target"), root).expect("alias");
    assert_eq!(
        Catalog::discover(&client, &roots, &Cancel::default())
            .await
            .expect_err("unsafe root")
            .cause,
        Error::Read(ReadFailure::Worker)
    );
    let mut tooling = Tooling::of(binary.to_str().expect("binary"));
    tooling.pin = Some("0".repeat(64));
    let wrong = Client::new(tooling, BTreeMap::new());
    assert_eq!(
        Catalog::discover(&wrong, &roots, &Cancel::default())
            .await
            .expect_err("wrong pin")
            .cause,
        Error::Pin
    );
}
