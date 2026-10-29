#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::events::collection::runtime;
use magi_proto::tooling::{Call, FileCapability, FileOperation, FileRevision};
use magi_testkit::Scratch;
use magi_tools::{Cancel, Uncancelled, holding::Context, supplier};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const TOOLS: &str = r#"casper.tool("probe", {description="probe", parameters={}, run=function(args)
 local done = casper.exec("sh", {"-c", args.command})
 return {said=done.out, failed=done.code ~= 0}
end})"#;
const PROBE: &str = r#"
printf '%s' "$$" > shell-pid
for f in /proc/$$/fd/*; do readlink "$f" || :; done > shell-fds
: > ready
n=0
while [ ! -f release ]; do n=$((n+1)); [ "$n" -lt 500 ] || exit 73; sleep 0.01; done
printf native-tool-result
"#;

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "supplier deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Cancelled(Arc<AtomicBool>);
impl Cancel for Cancelled {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

fn child(binary: &Path) -> Option<u32> {
    fs::read_dir("/proc")
        .expect("processes")
        .filter_map(Result::ok)
        .find_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let status = fs::read_to_string(entry.path().join("status")).ok()?;
            let parent = status
                .lines()
                .find_map(|line| line.strip_prefix("PPid:\t"))?;
            (parent.trim().parse::<u32>().ok()? == std::process::id()
                && fs::read_link(entry.path().join("exe")).ok()?.as_path() == binary)
                .then_some(pid)
        })
}

fn call(command: &str, root: &Path) -> Call {
    Call {
        tool: "probe".into(),
        args: json!({"command":command}),
        cwd: root.to_string_lossy().into_owned(),
        answered: None,
        revision: FileRevision::default(),
    }
}

fn exercise(mode: &str) {
    let root = std::env::current_dir().expect("fixture root");
    let binary = fs::canonicalize(std::env::var_os("CASPER_TEST_BINARY").expect("selected Casper"))
        .expect("binary");
    let enabled = !mode.ends_with("disabled");
    let legacy = mode.ends_with("legacy");
    let library = mode.starts_with("library");
    let scope = enabled.then(|| runtime::Scope::start().expect("collector"));
    if mode == "negotiate" {
        cancelled_negotiation(&binary, &root);
        supplier::shutdown();
        drop(scope);
        return;
    }
    let program = if legacy || !enabled {
        let wrapper = root.join("legacy");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nif [ \"$1\" = event-capabilities ]; then printf x >> '{}'; {}; fi\nexec '{}' \"$@\"\n",
                root.join("queries").display(),
                if legacy { "exit 1" } else { ":" },
                binary.display()
            ),
        )
        .expect("legacy wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700))
            .expect("wrapper executable");
        wrapper
    } else {
        binary.clone()
    };
    let context = Context {
        cwd: Some(root.clone()),
        ..Default::default()
    };
    let requested = call(PROBE, &root);
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let selected = program.clone();
    let tool_context = context.clone();
    let task = std::thread::spawn(move || {
        supplier::run_context(
            selected.to_str().expect("program"),
            &requested,
            &tool_context,
            &Cancelled(flag),
        )
    });
    wait(|| root.join("ready").exists());
    let pid = child(&binary).expect("selected native Casper");
    let argv = fs::read(format!("/proc/{pid}/cmdline")).expect("native argv");
    assert_eq!(
        argv.split(|b| *b == 0)
            .any(|arg| arg == b"--event-bootstrap"),
        enabled && !legacy
    );
    let verb: &[u8] = if library { b"serve" } else { b"run" };
    assert!(argv.split(|b| *b == 0).any(|arg| arg == verb));
    assert!(
        !fs::read_to_string(root.join("shell-fds"))
            .expect("shell descriptors")
            .contains("socket:")
    );
    if enabled && !legacy {
        wait(|| {
            runtime::status()
                .summary
                .is_some_and(|summary| summary.remote_producers == 1 && summary.counts.accepted > 0)
        });
    }
    let cancelled_at = Instant::now();
    if mode.ends_with("cancel") {
        cancelled.store(true, Ordering::Release);
        assert!(task.join().expect("call thread").is_err());
    } else {
        fs::write(root.join("release"), b"release").expect("release");
        let result = task.join().expect("call thread").expect("tool result");
        assert!(!result.failed);
        assert_eq!(result.said, "native-tool-result");
    }
    let shell = fs::read_to_string(root.join("shell-pid")).expect("shell pid");
    wait(|| {
        let gone = !Path::new(&format!("/proc/{shell}")).exists();
        if mode.ends_with("cancel") {
            assert!(
                gone || cancelled_at.elapsed() < Duration::from_secs(2),
                "cancellation waited for the tool's own deadline"
            );
        }
        gone
    });
    if library {
        assert!(
            Path::new(&format!("/proc/{pid}")).exists(),
            "keeper thread lost its daemon"
        );
        let again = supplier::run_context(
            program.to_str().expect("program"),
            &call("printf second-result", &root),
            &context,
            &Uncancelled,
        )
        .expect("reused daemon");
        assert_eq!(again.said, "second-result");
        assert_eq!(child(&binary), Some(pid));
    } else {
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "native call not reaped"
        );
    }
    if mode == "enabled" {
        let mut guarded = call("printf forbidden > forbidden", &root);
        guarded.revision = FileRevision::native(
            &FileCapability {
                version: 1,
                operation: FileOperation::Read,
            },
            None,
        )
        .expect("native admission");
        let result = supplier::run_context(
            program.to_str().expect("program"),
            &guarded,
            &context,
            &Uncancelled,
        );
        assert!(result.is_err() || result.is_ok_and(|ran| ran.failed));
        assert!(
            !root.join("forbidden").exists(),
            "admission refusal executed the tool"
        );
    }
    let socket =
        library.then(|| root.join(format!("runtime/casper/magi-{}-0.sock", std::process::id())));
    if mode == "library-replaced" {
        let socket = socket.as_ref().expect("daemon socket");
        fs::rename(socket, socket.with_extension("saved")).expect("replace socket");
        fs::write(socket, b"replacement").expect("replacement file");
    }
    supplier::shutdown();
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "shutdown did not reap daemon"
    );
    if let Some(socket) = socket {
        if mode == "library-replaced" {
            assert_eq!(
                fs::read(socket).expect("replacement retained"),
                b"replacement"
            );
        } else {
            assert!(!socket.exists(), "owned socket retained after shutdown");
        }
    }
    if enabled {
        wait(|| {
            runtime::status()
                .summary
                .is_some_and(|summary| summary.remote_producers == 0)
        });
        assert_eq!(
            runtime::status().summary.expect("summary").counts.invalid,
            0
        );
        assert_eq!(runtime::status().unobserved_launches, u64::from(legacy));
    } else {
        assert_eq!(runtime::status().state, runtime::StatusKind::Disabled);
        assert!(
            !root.join("queries").exists(),
            "disabled collection queried capabilities"
        );
    }
    drop(scope);
}

fn cancelled_negotiation(binary: &Path, root: &Path) {
    let program = root.join("slow");
    fs::write(&program, format!("#!/bin/sh\nif [ \"$1\" = event-capabilities ]; then printf '%s' $$ > '{}'; sleep 0.2; exec '{}' event-capabilities; fi\nprintf launched > '{}'\nexec '{}' \"$@\"\n", root.join("probe-pid").display(), binary.display(), root.join("launched").display(), binary.display())).expect("slow capability");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).expect("executable");
    let requested = call("printf forbidden > forbidden", root);
    let context = Context {
        cwd: Some(root.to_owned()),
        ..Default::default()
    };
    let cancelled = Arc::new(AtomicBool::new(true));
    assert!(
        supplier::run_context(
            program.to_str().expect("program"),
            &requested,
            &context,
            &Cancelled(cancelled.clone())
        )
        .is_err()
    );
    assert!(!root.join("probe-pid").exists());
    cancelled.store(false, Ordering::Release);
    let flag = cancelled.clone();
    let task = std::thread::spawn(move || {
        supplier::run_context(
            program.to_str().expect("program"),
            &requested,
            &context,
            &Cancelled(flag),
        )
    });
    wait(|| fs::read_to_string(root.join("probe-pid")).is_ok_and(|pid| pid.parse::<u32>().is_ok()));
    cancelled.store(true, Ordering::Release);
    assert!(task.join().expect("negotiation thread").is_err());
    let pid = fs::read_to_string(root.join("probe-pid")).expect("query PID");
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(!root.join("launched").exists());
    assert!(!root.join("forbidden").exists());
    let summary = runtime::status().summary.expect("summary");
    assert_eq!(summary.remote_producers, 0);
    assert_eq!(summary.retired.producers, 0);
    assert_eq!(runtime::status().unobserved_launches, 0);
}

#[test]
fn native_supplier_collection_preserves_run_daemon_and_admission() {
    if let Ok(mode) = std::env::var("NERV_EVENT_SUPPLIER_CASE") {
        exercise(&mode);
        return;
    }
    let Some(selected) = std::env::var_os("CASPER_TEST_BINARY") else {
        magi_testkit::live::unavailable("CASPER_TEST_BINARY is not set");
        return;
    };
    let binary = fs::canonicalize(selected).expect("selected binary");
    for mode in [
        "disabled",
        "enabled",
        "legacy",
        "cancel",
        "library",
        "library-replaced",
        "library-disabled",
        "library-legacy",
        "library-cancel",
        "negotiate",
    ] {
        let root = Scratch::new("es", mode);
        for name in [
            "home",
            "config/casper",
            "runtime",
            "tmp",
            "cache",
            "data",
            "state",
        ] {
            fs::create_dir_all(root.join(name)).expect("directory");
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700))
                .expect("private mode");
        }
        fs::write(root.join("config/casper/tools.lua"), TOOLS).expect("tools");
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "native_supplier_collection_preserves_run_daemon_and_admission",
                "--nocapture",
            ])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH"))
            .env("CASPER_TEST_BINARY", &binary)
            .env("NERV_EVENT_SUPPLIER_CASE", mode)
            .env(
                "MAGI_TOOLS_DOOR",
                if mode.starts_with("library") {
                    "library"
                } else {
                    "command"
                },
            )
            .current_dir(&*root)
            .stdin(Stdio::null());
        for (key, path) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("TMPDIR", "tmp"),
        ] {
            command.env(key, root.join(path));
        }
        let mut child = magi_tools::native::spawn(command, None).expect("isolated fixture");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().expect("fixture status") {
                assert!(status.success(), "{mode}: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "{mode}: fixture deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
