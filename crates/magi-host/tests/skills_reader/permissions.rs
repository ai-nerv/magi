#![allow(clippy::expect_used)]

use super::*;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "MAGI_WORKFLOW_PERMISSION_CHILD";
const LANDLOCK: &str = "permissions::unreadable_workflows_fail_closed_under_landlock";
const BWRAP: &str = "permissions::unreadable_workflows_fail_closed_under_bubblewrap";

struct Process(Option<Child>);

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = rustix::process::kill_process_group(
                rustix::process::Pid::from_child(child),
                rustix::process::Signal::Kill,
            );
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Mode(PathBuf, std::fs::Permissions);

impl Mode {
    fn deny(path: &Path) -> Self {
        let original = std::fs::metadata(path)
            .expect("original mode")
            .permissions();
        let held = Self(path.to_owned(), original);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).expect("deny reads");
        held
    }
}

impl Drop for Mode {
    fn drop(&mut self) {
        std::fs::set_permissions(&self.0, self.1.clone()).expect("restore fixture mode");
    }
}

fn run(case: &str, bwrap: bool) {
    let Some(binary) = selected() else { return };
    if std::env::var(CHILD).as_deref() == Ok(case) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(check(&binary, bwrap));
        return;
    }
    let private = Scratch::new("wp", "namespace");
    let mut command = Command::new("bwrap");
    command
        .process_group(0)
        .args([
            "--unshare-user",
            "--uid",
            "0",
            "--gid",
            "0",
            "--cap-add",
            "ALL",
            "--cap-drop",
            "CAP_DAC_OVERRIDE",
            "--cap-drop",
            "CAP_DAC_READ_SEARCH",
            "--bind",
            "/",
            "/",
            "--unshare-pid",
            "--unshare-net",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--die-with-parent",
            "--",
        ])
        .arg(std::env::current_exe().expect("test executable"))
        .args(["--exact", case, "--nocapture"])
        .env(CHILD, case)
        .env("TMPDIR", &*private)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut process = Process(Some(command.spawn().expect("permission fixture")));
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = process
            .0
            .as_mut()
            .expect("child")
            .try_wait()
            .expect("status")
        {
            process.0 = None;
            assert!(status.success(), "permission fixture failed: {status}");
            break;
        }
        assert!(Instant::now() < until, "permission fixture deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn check(binary: &Path, bwrap: bool) {
    let fixture = Scratch::new("wr", if bwrap { "bwrap" } else { "landlock" });
    let roots = Roots::new(&fixture.join("work"), None, &fixture.join("config")).expect("roots");
    let project = &roots.sources()[0].root;
    let source = source("review", "Private review", "PROJECT-PRIVATE-BODY");
    put(project, "review", &source);
    put(
        &roots.sources()[1].root,
        "review",
        &super::source("review", "Shadow", "NO-FALLBACK"),
    );
    let file = project.join("review/SKILL.md");
    let client = client(&fixture, binary, bwrap);
    let cancel = Cancel::default();
    let catalog = Catalog::discover(&client, &roots, &cancel)
        .await
        .expect("readable catalog");
    assert_eq!(
        catalog
            .load(&client, "review", &cancel)
            .await
            .expect("readable source")
            .document
            .body,
        "PROJECT-PRIVATE-BODY"
    );
    let denied = Mode::deny(&file);
    assert_eq!(
        std::fs::read(&file)
            .expect_err("fixture cannot bypass mode")
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        catalog.load(&client, "review", &cancel).await,
        Err(Error::Read(ReadFailure::Unreadable))
    );
    let unreadable = Catalog::discover(&client, &roots, &cancel)
        .await
        .expect("visible entry failure");
    let selected = unreadable
        .entries()
        .find(|entry| entry.name == "review")
        .expect("entry retained");
    assert_eq!(selected.source.class, SourceClass::Project);
    assert_eq!(selected.metadata, Err(Error::Read(ReadFailure::Unreadable)));
    assert_eq!(
        unreadable.load(&client, "review", &cancel).await,
        Err(Error::Read(ReadFailure::Unreadable))
    );
    drop(denied);
    let denied = Mode::deny(project);
    assert_eq!(
        std::fs::read_dir(project)
            .expect_err("fixture root cannot bypass mode")
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let failure = Catalog::discover(&client, &roots, &cancel)
        .await
        .expect_err("root failure is not empty");
    assert_eq!(failure.location.class, SourceClass::Project);
    assert_eq!(failure.cause, Error::Read(ReadFailure::Unreadable));
    drop(denied);
    assert_eq!(
        Catalog::discover(&client, &roots, &cancel)
            .await
            .expect("recovered catalog")
            .load(&client, "review", &cancel)
            .await
            .expect("recovered source")
            .document
            .body,
        "PROJECT-PRIVATE-BODY"
    );
    assert_eq!(
        std::fs::read_to_string(file).expect("source unchanged"),
        source
    );
}

#[test]
fn unreadable_workflows_fail_closed_under_landlock() {
    run(LANDLOCK, false);
}

#[test]
fn unreadable_workflows_fail_closed_under_bubblewrap() {
    run(BWRAP, true);
}
