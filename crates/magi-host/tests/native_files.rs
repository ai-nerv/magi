#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{session::Session, turn::Backend};
use magi_model::scratch::Scratch;
use magi_proto::{Entry, SessionId};
use magi_testkit::{
    Mind,
    mind::{call_lines, stop_line, text_line},
};
use magi_tools::{
    Registry,
    supplier::{SuppliedTool, Tooling},
};
use serde_json::{Value, json};
use std::os::unix::{
    fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    process::CommandExt,
};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const CASE: &str =
    "native_revisions_keep_agent_ownership_through_actual_casper_both_doors_and_backends";
const CHILD: &str = "MAGI_NATIVE_FILES_CHILD";
const SHIPPED: &str = "MAGI_NATIVE_FILES_SHIPPED";
const RECOVERY: &str = "MAGI_NATIVE_SOCKET_RECOVERY";
const RECOVERY_CASE: &str =
    "native_reply_loss_after_commit_requires_actual_rereads_on_both_doors_and_backends";

#[path = "native_files/recovery.rs"]
mod recovery;

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = i32::try_from(self.0.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn executable(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

fn outer(
    casper: &Path,
    backend: &Path,
    bubblewrap: bool,
    door: &str,
    shipped: Option<&Path>,
    recovery: bool,
) {
    use std::io::Read;
    let dir = Scratch::new("magi-casper-files", &format!("{bubblewrap}-{door}"));
    for name in [
        "home", "config", "data", "state", "cache", "run", "tmp", "var-tmp", "work", "bin",
    ] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir.join(name))
            .expect("private fixture");
    }
    std::fs::set_permissions(dir.join("var-tmp"), std::fs::Permissions::from_mode(0o1777))
        .expect("sticky private anchor");
    std::fs::create_dir(dir.join("config/casper")).expect("fixture declarations");
    if let Some(source) = shipped {
        std::fs::copy(source, dir.join("config/casper/tools.lua"))
            .expect("selected shipped declarations");
    } else {
        std::fs::write(dir.join("config/casper/tools.lua"), r#"
casper.file_tool("look",{operation="read",description="native snapshot",parameters={type="object",properties={path={type="string"},limit={type="integer"}},required={"path"}},script=true})
casper.file_tool("change",{operation="edit",description="native edit",parameters={type="object",properties={path={type="string"},old={type="string"},new={type="string"}},required={"path","old","new"}}})
casper.file_tool("store",{operation="write",description="native write",parameters={type="object",properties={path={type="string"},contents={type="string"}},required={"path","contents"}}})
"#).expect("native declarations");
    }
    let metadata = std::fs::metadata(backend).expect("selected backend");
    assert_eq!(
        metadata.mode() & 0o222,
        0,
        "test backend must already be read-only"
    );
    assert_eq!(
        metadata.nlink(),
        1,
        "test backend must have no hardlink aliases"
    );
    let mut paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .filter(|path| !path.join("bwrap").is_file())
        .collect();
    if bubblewrap {
        paths.insert(0, backend.parent().expect("backend parent").to_owned());
    }
    let mut command = Command::new(backend);
    command
        .process_group(0)
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--uid",
            "0",
            "--gid",
            "0",
            "--ro-bind",
            "/",
            "/",
        ])
        .args([
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--cap-add",
            "ALL",
            "--bind",
        ])
        .arg(&*dir)
        .arg(&*dir)
        .arg("--bind")
        .arg(dir.join("var-tmp"))
        .arg("/var/tmp")
        .args(["--die-with-parent", "--new-session", "--chdir"])
        .arg(dir.join("work"))
        .arg("--")
        .arg(std::env::current_exe().expect("host test executable"))
        .args([
            "--exact",
            if recovery { RECOVERY_CASE } else { CASE },
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", std::env::join_paths(paths).expect("backend PATH"))
        .env(CHILD, if bubblewrap { "bubblewrap" } else { "landlock" })
        .env(SHIPPED, if shipped.is_some() { "1" } else { "0" })
        .env(RECOVERY, if recovery { "1" } else { "0" })
        .env("MAGI_NATIVE_FILES_ROOT", &*dir)
        .env("CASPER_TEST_BINARY", casper)
        .env("MAGI_REQUIRE_LIVE", "1")
        .env("MAGI_TOOLS_DOOR", door)
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("TMPDIR", dir.join("tmp"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = Process(command.spawn().expect("isolated actual family integration"));
    let out = child.0.stdout.take().expect("child output");
    let err = child.0.stderr.take().expect("child errors");
    let output = std::thread::spawn(move || {
        let mut held = Vec::new();
        out.take(1024 * 1024)
            .read_to_end(&mut held)
            .expect("bounded output");
        held
    });
    let errors = std::thread::spawn(move || {
        let mut held = Vec::new();
        err.take(1024 * 1024)
            .read_to_end(&mut held)
            .expect("bounded errors");
        held
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.0.try_wait().expect("fixture process") {
            break status;
        }
        assert!(Instant::now() < deadline, "actual family fixture deadline");
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = output.join().expect("stdout");
    let errors = errors.join().expect("stderr");
    assert!(
        status.success(),
        "backend={bubblewrap} door={door} stdout={} stderr={}",
        String::from_utf8_lossy(&output),
        String::from_utf8_lossy(&errors)
    );
}

fn backend(mind: &Mind, cwd: &Path) -> Backend {
    Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        tooling: Default::default(),
        cwd: cwd.into(),
        model: "fixture/native-files".into(),
        mind: mind.program().display().to_string(),
        wants: Default::default(),
        context_window: Some(200_000),
        max_output: None,
        system: None,
        confine: false,
        isolate: true,
        grants: Vec::new(),
        environ: Default::default(),
        helpers: Default::default(),
        deciders: Vec::new(),
    }
}

fn selected_tool(name: &str) -> &str {
    if std::env::var(SHIPPED).as_deref() == Ok("1") {
        match name {
            "look" => "read",
            "change" => "edit",
            "store" => "write",
            _ => name,
        }
    } else {
        name
    }
}

async fn call(
    session: &Mutex<Session>,
    registry: &Registry,
    ops: &dyn magi_tools::Ops,
    cwd: &Path,
    tool: &str,
    args: Value,
) -> magi_proto::ToolResult {
    let lines = call_lines("reused-native-id", selected_tool(tool), &args.to_string());
    let text = text_line("finished");
    let stop = stop_line();
    let mind = Mind::turns(
        "actual-native-files",
        &[&[&lines[0], &lines[1], &lines[2]], &[&text, &stop]],
    );
    let scribe = Arc::new(Mutex::new(None));
    magi_host::turn::run(session, &backend(&mind, cwd), registry, ops, &scribe)
        .await
        .expect("actual tool turn");
    session
        .lock()
        .await
        .entries()
        .iter()
        .rev()
        .find_map(|entry| match entry {
            Entry::Tool {
                result: Some(result),
                ..
            } => Some(result.clone()),
            _ => None,
        })
        .expect("actual paired tool result")
}

async fn integrated() {
    let root = PathBuf::from(std::env::var_os("MAGI_NATIVE_FILES_ROOT").expect("private root"));
    let cwd = root.join("work");
    let casper = std::env::var_os("CASPER_TEST_BINARY").expect("selected actual Casper");
    assert_eq!(
        executable("bwrap").is_some(),
        std::env::var(CHILD).as_deref() == Ok("bubblewrap")
    );
    let wrapper = root.join("bin/tools-role");
    let actual = magi_proto::permit::quoted(Path::new(&casper).to_str().expect("Casper path"));
    let marker = magi_proto::permit::quoted(root.join("drop-next").to_str().expect("marker"));
    let cat = magi_proto::permit::quoted(
        executable("cat")
            .expect("fixture cat")
            .to_str()
            .expect("cat path"),
    );
    let rm = magi_proto::permit::quoted(
        executable("rm")
            .expect("fixture rm")
            .to_str()
            .expect("rm path"),
    );
    let proxy = if std::env::var(RECOVERY).as_deref() == Ok("1") {
        let path = std::env::current_exe()
            .expect("test binary")
            .parent()
            .and_then(Path::parent)
            .expect("Cargo profile directory")
            .join("examples/file-reply-proxy");
        assert!(
            path.is_file(),
            "owner recipe must build the exact reply proxy"
        );
        format!(
            "if [ \"$1\" = run ] || [ \"$1\" = serve ]; then exec {} {actual} {} \"$@\"; fi\n",
            magi_proto::permit::quoted(path.to_str().expect("proxy path")),
            magi_proto::permit::quoted(root.to_str().expect("proxy root"))
        )
    } else {
        String::new()
    };
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n{proxy}if [ \"$1\" = run ] && [ -f {marker} ]; then\n  {rm} -- {marker}\n  {actual} \"$@\" | {{ IFS= read -r header; printf '%s\\n' \"$header\"; {cat} >/dev/null; }}\nelse\n  exec {actual} \"$@\"\nfi\n",
            cwd.join("doors").display(),
        ),
    )
    .expect("actual process wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))
        .expect("wrapper mode");
    let tooling = Tooling::of(wrapper.to_str().expect("wrapper path"));
    let mut registry = Registry::new();
    for tool in SuppliedTool::all(
        &tooling,
        Arc::new(magi_tools::question::Unanswered),
        Arc::new(magi_tools::holding::Screenless),
        Arc::new(magi_tools::holding::Incurious),
        &Default::default(),
    ) {
        registry.register(Box::new(tool));
    }
    assert!(
        registry.get(selected_tool("look")).is_some()
            && registry.get(selected_tool("change")).is_some(),
        "actual native registrations unavailable"
    );
    let grants = vec![magi_proto::permit::Grant {
        verb: "write".into(),
        scope: magi_proto::permit::Scope::Directory {
            path: cwd.display().to_string(),
        },
    }];
    let ops = magi_tools::ops::Real::gated(
        cwd.clone(),
        magi_tools::permit::Ledger::with(grants),
        Arc::new(magi_tools::approve::AllowAll),
    )
    .isolating(true);
    let parent = Mutex::new(Session::recorded(SessionId::new("parent"), Vec::new()));
    let sibling = Mutex::new(Session::recorded(SessionId::new("sibling"), Vec::new()));
    if std::env::var(RECOVERY).as_deref() == Ok("1") {
        recovery::verify(&root, &parent, &sibling, &registry, &ops).await;
        return;
    }
    let original = format!(
        "HEAD\n{}\nLEFT original\nRIGHT original\nTAIL λ\n",
        "x".repeat(400 * 1024)
    );
    std::fs::write(cwd.join("source"), &original).expect("large source fixture");
    let args = json!({"path":"source","limit":1});
    let large = format!("{}\nPREVIEW tail λ\n", "x".repeat(2 * 1024 * 1024));
    std::fs::write(cwd.join("preview"), &large).expect("readonly large source");
    let preview = call(
        &parent,
        &registry,
        &ops,
        &cwd,
        "look",
        json!({"path":"preview","offset":2,"limit":1}),
    )
    .await;
    assert!(
        !preview.is_error && preview.output.starts_with("PREVIEW tail λ"),
        "{}",
        preview.output
    );
    assert!(
        preview.output.contains("lines 2-2 of 2"),
        "{}",
        preview.output
    );
    let no_receipt = call(
        &parent,
        &registry,
        &ops,
        &cwd,
        "change",
        json!({"path":"preview","old":"PREVIEW","new":"unsafe"}),
    )
    .await;
    assert!(
        no_receipt.is_error && no_receipt.output.contains("delivered observation"),
        "{}",
        no_receipt.output
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("preview")).expect("unmodified preview"),
        large
    );
    let seen = call(&sibling, &registry, &ops, &cwd, "look", args.clone()).await;
    assert!(!seen.is_error, "{}", seen.output);
    let unobserved = call(
        &parent,
        &registry,
        &ops,
        &cwd,
        "change",
        json!({"path":"source","old":"LEFT original","new":"LEFT changed"}),
    )
    .await;
    assert!(
        unobserved.is_error && unobserved.output.contains("delivered observation"),
        "{}",
        unobserved.output
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("source")).expect("unchanged"),
        original
    );
    let read = call(&parent, &registry, &ops, &cwd, "look", args.clone()).await;
    assert!(!read.is_error, "{}", read.output);
    let left = call(
        &parent,
        &registry,
        &ops,
        &cwd,
        "change",
        json!({"path":"source","old":"LEFT original","new":"LEFT changed"}),
    )
    .await;
    assert!(!left.is_error, "{}", left.output);
    let own = call(
        &parent,
        &registry,
        &ops,
        &cwd,
        "change",
        json!({"path":"source","old":"TAIL λ","new":"TAIL kept λ"}),
    )
    .await;
    assert!(
        !own.is_error,
        "post-mutation receipt was not delivered: {}",
        own.output
    );
    let stale = call(
        &sibling,
        &registry,
        &ops,
        &cwd,
        "change",
        json!({"path":"source","old":"RIGHT original","new":"RIGHT changed"}),
    )
    .await;
    assert!(
        stale.is_error && stale.output.contains("stale"),
        "{}",
        stale.output
    );
    assert!(
        !call(&sibling, &registry, &ops, &cwd, "look", args)
            .await
            .is_error
    );
    let right = call(
        &sibling,
        &registry,
        &ops,
        &cwd,
        "change",
        json!({"path":"source","old":"RIGHT original","new":"RIGHT changed"}),
    )
    .await;
    assert!(!right.is_error, "{}", right.output);
    let expected = original
        .replace("LEFT original", "LEFT changed")
        .replace("RIGHT original", "RIGHT changed")
        .replace("TAIL λ", "TAIL kept λ");
    assert_eq!(
        std::fs::read_to_string(cwd.join("source")).expect("actual committed bytes"),
        expected
    );
    let create = call(
        &parent,
        &registry,
        &ops,
        &cwd,
        "store",
        json!({"path":"created","contents":"created once\n"}),
    )
    .await;
    assert!(!create.is_error, "{}", create.output);
    let other = call(
        &sibling,
        &registry,
        &ops,
        &cwd,
        "store",
        json!({"path":"created","contents":"must not overwrite\n"}),
    )
    .await;
    assert!(
        other.is_error,
        "unobserved existing write was not no-replace"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("created")).expect("created once"),
        "created once\n"
    );
    if std::env::var("MAGI_TOOLS_DOOR").as_deref() == Ok("command") {
        std::fs::write(root.join("drop-next"), "lose only the actual family reply")
            .expect("reply-loss barrier");
        let lost = call(
            &parent,
            &registry,
            &ops,
            &cwd,
            "store",
            json!({"path":"recover-created","contents":"invalid\u{0}"}),
        )
        .await;
        assert!(
            lost.is_error
                && lost
                    .output
                    .starts_with("Native file mutation outcome is unknown"),
            "{}",
            lost.output
        );
        assert!(
            !cwd.join("recover-created").exists(),
            "invalid native text was written"
        );
        let blocked = call(
            &parent,
            &registry,
            &ops,
            &cwd,
            "store",
            json!({"path":"recover-created","contents":"must wait"}),
        )
        .await;
        assert!(
            blocked.is_error && blocked.output.contains("reread"),
            "{}",
            blocked.output
        );
        let resolved = call(
            &parent,
            &registry,
            &ops,
            &cwd,
            "look",
            json!({"path":"recover-created"}),
        )
        .await;
        assert!(
            !resolved.is_error && resolved.output.contains("is absent"),
            "{}",
            resolved.output
        );
        let edit = call(
            &parent,
            &registry,
            &ops,
            &cwd,
            "change",
            json!({"path":"recover-created","old":"x","new":"y"}),
        )
        .await;
        assert!(
            edit.is_error && edit.output.contains("delivered observation"),
            "absence granted overwrite"
        );
        let recovered = call(
            &parent,
            &registry,
            &ops,
            &cwd,
            "store",
            json!({"path":"recover-created","contents":"recovered once\n"}),
        )
        .await;
        assert!(!recovered.is_error, "{}", recovered.output);
        assert_eq!(
            std::fs::read_to_string(cwd.join("recover-created"))
                .expect("actual recovered creation"),
            "recovered once\n"
        );
        assert!(!root.join("drop-next").exists());
    }
    let doors = std::fs::read_to_string(cwd.join("doors")).expect("actual process doors");
    if std::env::var("MAGI_TOOLS_DOOR").as_deref() == Ok("library") {
        assert_eq!(
            doors.lines().filter(|line| *line == "serve").count(),
            1,
            "{doors}"
        );
        assert!(
            !doors.lines().any(|line| line == "run"),
            "socket test silently fell back to command: {doors}"
        );
    } else {
        assert!(
            doors.lines().filter(|line| *line == "run").count() >= 9,
            "{doors}"
        );
        assert!(!doors.lines().any(|line| line == "serve"), "{doors}");
    }
}

#[tokio::test]
async fn native_revisions_keep_agent_ownership_through_actual_casper_both_doors_and_backends() {
    if std::env::var_os(CHILD).is_some() {
        integrated().await;
        return;
    }
    let Some(casper) = std::env::var_os("CASPER_TEST_BINARY").map(PathBuf::from) else {
        magi_testkit::live::unavailable(
            "actual native file integration requires selected CASPER_TEST_BINARY",
        );
        return;
    };
    assert!(
        casper.is_absolute() && casper.is_file(),
        "selected Casper must be an absolute executable artifact"
    );
    let Some(bwrap) = executable("bwrap") else {
        magi_testkit::live::unavailable(
            "actual native file integration requires bubblewrap for private namespaces",
        );
        return;
    };
    for backend in [false, true] {
        for door in ["command", "library"] {
            outer(&casper, &bwrap, backend, door, None, false);
        }
    }
}

#[tokio::test]
async fn shipped_revisions_keep_agent_ownership_through_actual_casper_both_doors_and_backends() {
    let Some(casper) = std::env::var_os("CASPER_TEST_BINARY").map(PathBuf::from) else {
        magi_testkit::live::unavailable(
            "shipped native integration requires selected CASPER_TEST_BINARY",
        );
        return;
    };
    let Some(tools) = std::env::var_os("CASPER_TEST_TOOLS").map(PathBuf::from) else {
        magi_testkit::live::unavailable(
            "shipped native integration requires explicit selected CASPER_TEST_TOOLS",
        );
        return;
    };
    assert!(
        casper.is_absolute() && casper.is_file(),
        "selected actual Casper"
    );
    assert!(
        tools.is_absolute() && tools.is_file(),
        "selected actual declarations"
    );
    let Some(bwrap) = executable("bwrap") else {
        magi_testkit::live::unavailable("shipped native integration requires bubblewrap");
        return;
    };
    for backend in [false, true] {
        for door in ["command", "library"] {
            outer(&casper, &bwrap, backend, door, Some(&tools), false);
        }
    }
}

#[tokio::test]
async fn native_reply_loss_after_commit_requires_actual_rereads_on_both_doors_and_backends() {
    if std::env::var_os(CHILD).is_some() {
        integrated().await;
        return;
    }
    let Some(casper) = std::env::var_os("CASPER_TEST_BINARY").map(PathBuf::from) else {
        magi_testkit::live::unavailable("reply-loss integration requires selected Casper");
        return;
    };
    let Some(tools) = std::env::var_os("CASPER_TEST_TOOLS").map(PathBuf::from) else {
        magi_testkit::live::unavailable("reply-loss integration requires selected declarations");
        return;
    };
    let Some(bwrap) = executable("bwrap") else {
        magi_testkit::live::unavailable("reply-loss integration requires private namespaces");
        return;
    };
    assert!(casper.is_absolute() && casper.is_file() && tools.is_absolute() && tools.is_file());
    for backend in [false, true] {
        for door in ["command", "library"] {
            outer(&casper, &bwrap, backend, door, Some(&tools), true);
        }
    }
}
