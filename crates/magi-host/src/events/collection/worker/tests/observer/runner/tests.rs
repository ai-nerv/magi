use super::*;
use crate::events::collection::worker::tests::native_child::{descendants, instance, selected};

#[test]
fn reaping_uses_one_deadline_even_after_it_expires() {
    let mut reaper = std::mem::ManuallyDrop::new(Reaper(0, None));
    let first = reaper.deadline();
    assert_eq!(reaper.deadline(), first);
    let expired = Instant::now() - Duration::from_secs(1);
    reaper.1 = Some(expired);
    assert_eq!(reaper.deadline(), expired);
}
use magi_testkit::{Mind, Scratch};
use std::os::unix::fs::{PermissionsExt, symlink};

fn request(output: PathBuf) -> Request {
    Request {
        schema: 1,
        mode: Mode::Stall,
        output,
        timeout_ms: 10_000,
        args: vec!["-p".into(), "literal request".into()],
    }
}

#[test]
fn native_marker_detection_preserves_literal_arguments_after_the_separator() {
    for args in [
        vec!["-p", "--", "--headless"],
        vec!["--headless", "-p", "literal"],
    ] {
        let original: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let launched = command(Path::new("/selected/magi"), &original);
        let actual: Vec<_> = launched
            .get_args()
            .map(|arg| arg.to_str().expect("argv"))
            .collect();
        let mut expected = vec!["--event-child", "--agent-event-bootstrap"];
        if args.first() == Some(&"-p") {
            expected.push("--headless");
        }
        expected.extend(args);
        assert_eq!(actual, expected);
    }
}

#[test]
fn launch_requests_are_bounded_and_cannot_replace_the_bootstrap() {
    let valid = request("/unused".into());
    let value = serde_json::to_value(&valid).expect("request");
    assert!(valid.validate().is_ok());
    for (key, change) in [
        ("schema", json!(2)),
        ("mode", json!("auto")),
        ("output", json!("relative")),
        ("timeout_ms", json!(0)),
        ("timeout_ms", json!(60_001)),
        ("args", json!([])),
        ("args", json!(["--event-child"])),
        ("args", json!(["--agent-event-bootstrap"])),
        ("args", json!(["\0"])),
        ("args", json!(["x".repeat(32_769)])),
        ("args", json!(vec!["x"; 65])),
        ("unknown", json!(true)),
    ] {
        let mut invalid = value.clone();
        invalid[key] = change;
        assert!(
            serde_json::from_value::<Request>(invalid).map_or(true, |r| r.validate().is_err()),
            "{key}"
        );
    }
}

#[test]
fn request_files_reject_aliases_duplicate_fields_and_excess_bytes() {
    let scratch = Scratch::new("observer", "input");
    let path = scratch.join("request");
    fs::write(
        &path,
        serde_json::to_vec(&request(scratch.join("out"))).expect("JSON"),
    )
    .expect("file");
    assert!(Request::read(&path).is_ok());
    symlink(&path, scratch.join("alias")).expect("alias");
    assert!(Request::read(&scratch.join("alias")).is_err());
    fs::write(&path, b"{\"schema\":1,\"schema\":1}").expect("duplicate");
    assert!(Request::read(&path).is_err());
    let fifo = scratch.join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("FIFO");
    assert!(Request::read(&fifo).is_err());
    assert!(digest(&fifo).is_err());
    fs::write(&path, vec![b' '; INPUT_LIMIT as usize + 1]).expect("oversize");
    assert!(Request::read(&path).is_err());
}

fn wait(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "observer native barrier: {label}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

struct Process {
    native: Native,
    directory: PathBuf,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.native.signal(rustix::process::Signal::Term);
        let end = Instant::now() + Duration::from_secs(3);
        while matches!(self.native.status(), Ok(None)) && Instant::now() < end {
            thread::sleep(Duration::from_millis(5));
        }
        if let Ok(bytes) = fs::read(self.directory.join("ready.json"))
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && let (Some(pid), Some(start)) = (
                value["native"]["pid"].as_u64(),
                value["native"]["start_time"].as_str(),
            )
        {
            let pid = u32::try_from(pid).expect("native PID");
            if instance(pid).as_deref() == Some(start)
                && let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
            {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::Kill);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum End {
    Finish,
    Signal(rustix::process::Signal),
    Timeout,
}

fn scenario(mode: Mode, end: End) {
    let cancel = matches!(end, End::Signal(_));
    let timeout = matches!(end, End::Timeout);
    let Some([magi, melchior, balthasar, casper]) = selected() else {
        return;
    };
    let root = Scratch::new(
        "observer",
        if cancel {
            "cancel"
        } else if mode == Mode::Stall {
            "stall"
        } else {
            "drain"
        },
    )
    .settling();
    for dir in ["c/magi", "c/casper", "r", "d", "s", "h", "t", "p/.git"] {
        fs::create_dir_all(root.join(dir)).expect("fixture directory");
    }
    fs::set_permissions(root.join("r"), fs::Permissions::from_mode(0o700)).expect("runtime");
    let mind = Mind::controlled(if timeout {
        "observer-native-timeout"
    } else if matches!(end, End::Signal(rustix::process::Signal::Int)) {
        "observer-native-int"
    } else if cancel {
        "observer-native-term"
    } else if mode == Mode::Stall {
        "observer-native-stall"
    } else {
        "observer-native-drain"
    });
    let router = root.join("router");
    fs::write(&router, format!("#!/bin/sh\ncase \"$1\" in\nmodels|ask|needs) exec {:?} \"$@\" ;;\nevent-capabilities) printf '%s\\n' '{{\"ok\":false}}' ;;\n*) exec {:?} \"$@\" ;;\nesac\n", mind.program(), melchior)).expect("router");
    fs::set_permissions(&router, fs::Permissions::from_mode(0o700)).expect("router permissions");
    magi_model::scratch::executable_ready(&router);
    fs::write(root.join("c/magi/init.lua"), format!("magi.events = true\nmagi.model = 'fake/one'\nmagi.helpers = {{ memory = false }}\nmagi.melchior = {router:?}\nmagi.memory = {balthasar:?}\nmagi.tools = {casper:?}\n")).expect("config");
    fs::write(root.join("c/casper/tools.lua"), "").expect("tools");
    let mut input = request(root.join("observation"));
    input.mode = mode;
    input.timeout_ms = if timeout { 2000 } else { 20_000 };
    fs::write(
        root.join("request"),
        serde_json::to_vec(&input).expect("request"),
    )
    .expect("input");
    let mut process = Process {
        native: Native(
            Command::new(std::env::current_exe().expect("observer executable"))
                .args(["--exact", ENTRY, "--nocapture", "--test-threads=1"])
                .env_clear()
                .process_group(0)
                .env(REQUEST, root.join("request"))
                .env("MAGI_TEST_BINARY", &magi)
                .env("PATH", std::env::var_os("PATH").expect("path"))
                .env("HOME", root.join("h"))
                .env("TMPDIR", root.join("t"))
                .env("XDG_CONFIG_HOME", root.join("c"))
                .env("XDG_RUNTIME_DIR", root.join("r"))
                .env("XDG_DATA_HOME", root.join("d"))
                .env("XDG_STATE_HOME", root.join("s"))
                .current_dir(root.join("p"))
                .stdin(Stdio::null())
                .stdout(output(&root.join("observer.stdout")).expect("output"))
                .stderr(output(&root.join("observer.stderr")).expect("error output"))
                .spawn()
                .expect("observer"),
            false,
        ),
        directory: input.output.clone(),
    };
    wait("startup and provider admission", || {
        assert!(
            process.native.status().expect("observer status").is_none(),
            "{}",
            fs::read_to_string(root.join("observer.stdout")).unwrap_or_default()
        );
        input.output.join("ready.json").exists() && mind.asked() == 1
    });
    let ready: serde_json::Value =
        serde_json::from_slice(&fs::read(input.output.join("ready.json")).expect("ready"))
            .expect("ready JSON");
    let pid = ready["native"]["pid"].as_u64().expect("PID") as u32;
    assert_eq!(
        fs::read_link(format!("/proc/{}/exe", process.native.0.id())).expect("observer image"),
        fs::canonicalize(std::env::current_exe().expect("test image")).expect("selected observer")
    );
    assert_ne!(
        pid,
        process.native.0.id(),
        "observer cannot impersonate selected Magi"
    );
    assert_eq!(
        fs::read_link(format!("/proc/{pid}/exe")).expect("actual selected executable"),
        magi
    );
    assert_eq!(
        ready["native"]["executable_sha256"],
        digest(&magi).expect("selected hash")
    );
    let owned = descendants(pid);
    if let End::Signal(signal) = end {
        process.native.signal(signal).expect("forward cancellation");
    } else if !timeout {
        mind.release(0);
    }
    let mut result = None;
    wait("observer reaping", || {
        result = process.native.status().expect("observer reap");
        result.is_some()
    });
    assert_eq!(
        result.expect("exit").success(),
        !timeout,
        "{}",
        fs::read_to_string(root.join("observer.stdout")).unwrap_or_default()
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(input.output.join("summary.json")).expect("summary"))
            .expect("summary JSON");
    assert_eq!(receipt["status"], if timeout { "FAIL" } else { "PASS" });
    assert_eq!(receipt["child_reaped"], true);
    assert_eq!(receipt["adopted_children_complete"], true, "{receipt}");
    assert_eq!(receipt["timed_out"], timeout);
    assert_eq!(receipt["native"], ready["native"]);
    assert_eq!(receipt["forwarded_signals"], if cancel { 1 } else { 0 });
    assert_eq!(mind.asked(), 1, "no observation-triggered retry");
    if mode == Mode::Stall {
        assert_eq!(receipt["before_release"]["counts"]["accepted"], 0);
    }
    if !cancel && !timeout {
        assert_eq!(receipt["child_exit_code"], 0);
        assert!(
            fs::read_to_string(input.output.join("stdout"))
                .expect("native output")
                .contains("-done")
        );
    }
    wait("selected descendant instances disappearing", || {
        owned
            .iter()
            .all(|(pid, start)| instance(*pid).as_ref() != Some(start))
    });
}

#[test]
fn selected_work_finishes_under_the_same_draining_and_stalled_parent() {
    scenario(Mode::Drain, End::Finish);
    scenario(Mode::Stall, End::Finish);
}

#[test]
fn selected_work_is_cancelled_and_reaped_when_the_observer_receives_signals() {
    scenario(Mode::Stall, End::Signal(rustix::process::Signal::Term));
    scenario(Mode::Stall, End::Signal(rustix::process::Signal::Int));
}

#[test]
fn selected_work_timeout_is_recorded_as_failure_and_reaped() {
    scenario(Mode::Stall, End::Timeout);
}
