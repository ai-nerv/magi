use super::observer::Gate as Paused;
use super::*;
use crate::events::collection::{branch::Branch, enrollment::Host, transport};
use magi_ipc::blocking::{FrameReader, FrameWriter};
use magi_proto::{HarnessEvent, UiCommand};
use magi_testkit::{Mind, Scratch};
use rustix::fd::OwnedFd;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::Read,
    os::unix::{fs::PermissionsExt, net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

struct Native(Child, bool);

impl Drop for Native {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
        }
        let _ = self.0.wait();
    }
}

pub(super) fn selected() -> Option<[PathBuf; 4]> {
    let paths: Option<Vec<_>> = [
        "MAGI_TEST_BINARY",
        "MELCHIOR_TEST_BINARY",
        "BALTHASAR_TEST_BINARY",
        "CASPER_TEST_BINARY",
    ]
    .into_iter()
    .map(|name| {
        std::env::var_os(name).map(|path| fs::canonicalize(path).expect("selected executable"))
    })
    .collect();
    let Some(paths) = paths else {
        magi_testkit::live::unavailable("selected native family required for child backpressure");
        return None;
    };
    Some(paths.try_into().expect("four selected executables"))
}

fn wait_native(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < end, "selected child barrier deadline");
        thread::sleep(Duration::from_millis(5));
    }
}

fn inspect(socket: &Path) -> Value {
    let mut stream = UnixStream::connect(socket).expect("inspect connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("deadline");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("deadline");
    FrameWriter::new(&mut stream)
        .write_blocking(&UiCommand::Inspect)
        .expect("inspect");
    FrameReader::new(stream.take(16388))
        .read_blocking()
        .expect("bounded status")
}

fn turn(socket: &Path, mind: &Mind) {
    let mut stream = UnixStream::connect(socket).expect("controller");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("deadline");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("deadline");
    let mut writer = FrameWriter::new(stream.try_clone().expect("writer"));
    let mut reader = FrameReader::new(&mut stream);
    writer
        .write_blocking(&UiCommand::AttachLatest {
            draws: false,
            key: magi_ipc::read_key(socket),
        })
        .expect("attach");
    assert!(matches!(
        reader.read_blocking::<HarnessEvent>().expect("snapshot"),
        HarnessEvent::SessionSnapshot { .. }
    ));
    writer
        .write_blocking(&UiCommand::SubmitTracked {
            request: 1,
            text: "useful work while observation is unavailable".into(),
            images: Vec::new(),
            aside: String::new(),
        })
        .expect("submit");
    let mut owner = None;
    let mut useful = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    for _ in 0..128 {
        assert!(Instant::now() < deadline, "turn completion deadline");
        let frame: Value = reader.read_blocking().expect("native turn event");
        useful |= frame.to_string().contains("backpressure-useful-result");
        if frame["event"] == "turn_started" && frame["request"] == 1 {
            owner = frame["owner"].as_u64();
        }
        if frame["event"] == "turn_finished" && owner.is_some() && frame["owner"].as_u64() == owner
        {
            assert!(
                useful,
                "useful native answer must precede its turn boundary"
            );
            assert_eq!(mind.asked(), 1, "one provider call, no observation retry");
            return;
        }
        assert_ne!(frame["event"], "prompt_refused", "{frame}");
    }
    panic!("bounded native turn did not finish");
}

fn probed(sender: super::super::super::Sender) -> (super::super::super::Sender, OwnedFd) {
    let lineage = sender.lineage();
    let (inner, owner, producer) = sender.into_fresh().expect("fresh sender");
    let fd = inner.into_socket();
    let probe = fd.try_clone().expect("close-on-exec socket probe");
    (
        super::super::super::Sender::new(
            transport::Sender::adopt(fd).expect("sender"),
            owner,
            producer,
        )
        .with_lineage(lineage),
        probe,
    )
}

#[allow(deprecated)]
fn full(probe: &OwnedFd) -> bool {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("readiness runtime");
    runtime.block_on(async {
        let descriptor = tokio::io::unix::AsyncFd::new(probe.try_clone().expect("probe duplicate"))
            .expect("capacity probe");
        match tokio::time::timeout(Duration::from_millis(1), descriptor.writable()).await {
            Ok(ready) => {
                drop(ready.expect("socket readiness"));
                false
            }
            Err(_) => true,
        }
    })
}

pub(super) fn instance(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

pub(super) fn descendants(parent: u32) -> Vec<(u32, String)> {
    let mut pids = vec![parent];
    let mut index = 0;
    while index < pids.len() {
        let pid = pids[index];
        for task in fs::read_dir(format!("/proc/{pid}/task"))
            .expect("native threads")
            .flatten()
        {
            if let Ok(children) = fs::read_to_string(task.path().join("children")) {
                for child in children
                    .split_whitespace()
                    .map(|n| n.parse().expect("child PID"))
                {
                    if !pids.contains(&child) {
                        pids.push(child);
                    }
                }
            }
        }
        assert!(pids.len() <= 32, "fixture process bound");
        index += 1;
    }
    pids.into_iter()
        .map(|pid| (pid, instance(pid).expect("live native instance")))
        .collect()
}

#[cfg(test)]
fn scenario(stalled: bool) {
    let Some([binary, melchior, balthasar, casper]) = selected() else {
        return;
    };
    let root = Scratch::new("evchild", if stalled { "stalled" } else { "full" }).settling();
    for directory in ["c/magi", "c/casper", "r", "d", "s", "h", "t", "p/.git"] {
        fs::create_dir_all(root.join(directory)).expect("fixture directory");
    }
    fs::set_permissions(root.join("r"), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    let mind = Mind::answering(
        if stalled {
            "event-child-stalled"
        } else {
            "event-child-full"
        },
        "backpressure-useful-result",
    );
    let router = root.join("router");
    fs::write(&router, format!("#!/bin/sh\ncase \"$1\" in\nmodels|ask|needs) exec {:?} \"$@\" ;;\nevent-capabilities) printf '%s\\n' '{{\"ok\":false}}' ;;\n*) exec {:?} \"$@\" ;;\nesac\n", mind.program(), melchior)).expect("synthetic boundary");
    fs::set_permissions(&router, fs::Permissions::from_mode(0o700)).expect("router mode");
    magi_model::scratch::executable_ready(&router);
    fs::write(root.join("c/magi/init.lua"), format!("magi.events = true\nmagi.model = 'fake/one'\nmagi.helpers = {{ memory = false }}\nmagi.melchior = {router:?}\nmagi.memory = {balthasar:?}\nmagi.tools = {casper:?}\n")).expect("config");
    fs::write(root.join("c/casper/tools.lua"), "").expect("empty tools");
    let paused = Paused::new(stalled, Duration::from_secs(30));
    let lineage = Lineage::root(id(2), id(3));
    let parent = Lineage {
        turn_id: Some(id(5)),
        ask_id: Some(id(6)),
        ..Lineage::root(id(2), id(4))
    };
    let (mut host, sender, control) =
        Host::pair(paused.worker().registrar(), lineage).expect("child admission");
    let (sender, probe) = if stalled {
        let (sender, probe) = probed(sender);
        (sender, Some(probe))
    } else {
        (sender, None)
    };
    let mut held = Vec::new();
    if !stalled {
        let registrar = paused.worker().registrar();
        while registrar.available() {
            held.push(registrar.open(Owner::Magi).expect("capacity reservation"));
        }
        assert_eq!(held.len(), MAX_PRODUCERS - 4);
        assert!(matches!(
            registrar.open(Owner::Magi),
            Err(RegisterError::Capacity)
        ));
        assert!(
            Host::pair(registrar, Lineage::root(id(2), id(7))).is_err(),
            "full root cannot admit another branch"
        );
    }
    let packet = Branch::new(sender, control, parent)
        .expect("branch")
        .prepare(File::open("/dev/null").expect("original input"))
        .expect("child packet");
    let socket = root.join("r/child.sock");
    let mut child = Native(
        Command::new(&binary)
            .args([
                "--event-child",
                "--agent-event-bootstrap",
                "--headless",
                "--socket",
            ])
            .arg(&socket)
            .process_group(0)
            .env_clear()
            .current_dir(root.join("p"))
            .env("PATH", std::env::var_os("PATH").expect("path"))
            .env("HOME", root.join("h"))
            .env("TMPDIR", root.join("t"))
            .env("XDG_CONFIG_HOME", root.join("c"))
            .env("XDG_RUNTIME_DIR", root.join("r"))
            .env("XDG_DATA_HOME", root.join("d"))
            .env("XDG_STATE_HOME", root.join("s"))
            .stdin(Stdio::from(packet))
            .stdout(File::create(root.join("out")).expect("stdout"))
            .stderr(File::create(root.join("err")).expect("stderr"))
            .spawn()
            .expect("selected Magi child"),
        false,
    );
    wait_native(|| {
        assert!(
            child.0.try_wait().expect("child status").is_none(),
            "{}",
            fs::read_to_string(root.join("err")).expect("stderr")
        );
        assert!(host.poll(), "root enrollment stays live");
        socket.exists() && socket.with_extension("sock.key").exists()
    });
    assert_eq!(
        fs::read_link(format!("/proc/{}/exe", child.0.id())).expect("actual image"),
        binary
    );
    let state = inspect(&socket);
    assert_eq!(state["events"]["state"], "forwarding");
    assert_eq!(state["events"]["lineage"], json!(lineage));
    assert_eq!(state["events"]["parent"], json!(parent));
    let owned = descendants(child.0.id());
    for binary in [&melchior, &balthasar] {
        assert!(
            owned
                .iter()
                .any(|(pid, _)| fs::read_link(format!("/proc/{pid}/exe"))
                    .is_ok_and(|image| image == *binary)),
            "selected native sibling must be owned"
        );
    }
    if let Some(probe) = &probe {
        for _ in 0..128 {
            assert_eq!(inspect(&socket)["events"]["state"], "forwarding");
        }
        wait_native(|| full(probe));
    }
    turn(&socket, &mind);
    if stalled {
        assert_eq!(
            paused.worker().readout().snapshot().counts.accepted,
            0,
            "useful work finished before collector release"
        );
        assert!(
            full(probe.as_ref().expect("probe")),
            "collector remained stalled throughout useful work"
        );
        paused.resume();
        wait(&paused.worker().readout(), |summary| {
            summary.reported_loss_producers > 0
        });
    }
    let pid = rustix::process::Pid::from_raw(child.0.id() as i32).expect("PID");
    rustix::process::kill_process(pid, rustix::process::Signal::Term).expect("native shutdown");
    wait_native(|| {
        host.poll();
        child.0.try_wait().expect("reap").is_some_and(|status| {
            assert!(status.success());
            true
        })
    });
    child.1 = true;
    wait_native(|| {
        owned
            .iter()
            .all(|(pid, start)| instance(*pid).as_ref() != Some(start))
    });
    drop(child);
    drop(host);
    drop(probe);
    drop(held);
    wait(&paused.worker().readout(), |summary| {
        summary.remote_producers == 0
    });
    let summary = paused.finish();
    assert_eq!(summary.counts.invalid, 0);
    assert!(!summary.worker_failed && !summary.shutdown_overrun);
    assert!(
        summary.incomplete(),
        "saturation must never become a loss-free receipt"
    );
    if stalled {
        assert!(summary.retired.transport_loss.full > 0);
    } else {
        assert!(summary.counts.refused_producers >= 2);
    }
}

#[test]
fn selected_child_finishes_a_native_turn_with_stalled_full_event_transport() {
    scenario(true);
}

#[test]
fn selected_child_finishes_a_native_turn_while_root_admission_is_full() {
    scenario(false);
}
