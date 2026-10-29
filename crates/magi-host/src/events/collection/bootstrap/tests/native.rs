use super::*;
use rustix::fd::AsRawFd;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
};

const MODE: &str = "MAGI_TEST_EVENT_BOOTSTRAP_STAGE";
const TARGETS: &str = "MAGI_TEST_EVENT_BOOTSTRAP_TARGETS";
const INPUT: &str = "MAGI_TEST_EVENT_BOOTSTRAP_INPUT";
const CASE: &str = "events::collection::bootstrap::tests::native::native_exec_restores_stdin_and_excludes_event_descriptors";
const PAYLOAD: &[u8] = b"native-input-must-not-be-bootstrap-or-event-data";

fn target(fd: impl AsFd) -> PathBuf {
    std::fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd()))
        .expect("descriptor identity")
}

fn command(stage: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("selected test executable"));
    command
        .args(["--exact", CASE, "--nocapture"])
        .env(MODE, stage)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    command
}

struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn joined(mut command: Command) {
    let mut child = Reaped(command.spawn().expect("native child"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait().expect("wait child") {
            assert!(status.success(), "native bootstrap child failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "native bootstrap child deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut output = String::new();
    child
        .0
        .stdout
        .take()
        .expect("output")
        .take(4097)
        .read_to_string(&mut output)
        .expect("read output");
    assert!(output.len() <= 4096);
    assert!(output.contains("test result: ok. 1 passed; 0 failed; 0 ignored;"));
}

pub(in crate::events::collection) fn isolate(case: &str) -> bool {
    const KEY: &str = "MAGI_TEST_BOOTSTRAP_ISOLATED_CASE";
    if std::env::var(KEY).ok().as_deref() == Some(case) {
        return false;
    }
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args(["--exact", case, "--nocapture"])
        .env(KEY, case)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    joined(command);
    true
}

#[test]
fn native_exec_restores_stdin_and_excludes_event_descriptors() {
    match std::env::var(MODE).ok().as_deref() {
        Some("grandchild") => {
            let targets: Vec<_> = std::env::var(TARGETS)
                .expect("targets")
                .split(',')
                .map(PathBuf::from)
                .collect();
            for entry in std::fs::read_dir("/proc/self/fd").expect("descriptors") {
                if let Ok(link) = std::fs::read_link(entry.expect("entry").path()) {
                    assert!(!targets.contains(&link), "observation descriptor inherited");
                }
            }
            assert_eq!(
                target(std::io::stdin()),
                PathBuf::from(std::env::var_os(INPUT).expect("original input"))
            );
            return;
        }
        Some(stage @ ("child" | "forwarded")) => {
            let mut sender = receive_stdin(Owner::Magi).expect("native bootstrap");
            assert_eq!(
                target(std::io::stdin()),
                PathBuf::from(std::env::var_os(INPUT).expect("original input"))
            );
            let mut body = Vec::new();
            std::io::stdin()
                .take(129)
                .read_to_end(&mut body)
                .expect("original stdin");
            assert_eq!(body, PAYLOAD);
            let mut grandchild = command("grandchild");
            grandchild.stdin(Stdio::inherit());
            joined(grandchild);
            if stage == "forwarded" {
                use crate::events::{self, ClientOutcome, Correlation, Emitted, Fact};
                let forwarder =
                    super::super::super::Forwarder::activate(sender).expect("bound forwarder");
                assert_eq!(
                    events::emit(|| (
                        Correlation::default(),
                        Fact::Client {
                            outcome: ClientOutcome::Connected
                        }
                    )),
                    Emitted::Queued
                );
                let deadline = Instant::now() + Duration::from_secs(3);
                while events::PROCESS
                    .get()
                    .expect("process binding")
                    .loss
                    .dequeued
                    .load(std::sync::atomic::Ordering::Acquire)
                    != 1
                {
                    assert!(Instant::now() < deadline, "native forwarding deadline");
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(!forwarder.shutdown().expect("joined forwarder").incomplete());
            } else {
                assert_eq!(sender.send(&event(sender.identity())), Sent::Queued);
                assert_eq!(
                    sender.report(Loss::default(), true),
                    Report::Sent(Sent::Queued)
                );
            }
            return;
        }
        None => {}
        _ => panic!("invalid fixture stage"),
    }
    for stage in ["child", "forwarded"] {
        parent(stage);
    }
}

fn parent(stage: &str) {
    let (mut collector, Opened { sender, lifetime }) = setup();
    let (inner, owner, identity) = sender.into_fresh().expect("fresh sender");
    let socket = inner.into_socket();
    let event_target = target(&socket);
    let sender = Sender::new(
        transport::Sender::adopt(socket).expect("sender"),
        owner,
        identity,
    );
    let (input, mut writer) = UnixStream::pair().expect("original input");
    writer.write_all(PAYLOAD).expect("input body");
    writer
        .shutdown(std::net::Shutdown::Write)
        .expect("input eof");
    let input_target = target(&input);
    let bootstrap = prepare(sender, input).expect("bootstrap");
    let targets = format!(
        "{},{}",
        event_target.display(),
        target(&bootstrap).display()
    );
    let mut child = command(stage);
    child
        .env(TARGETS, targets)
        .env(INPUT, input_target)
        .stdin(Stdio::from(bootstrap));
    joined(child);
    drop(lifetime);
    collector.poll();
    collector.poll();
    let summary = collector.close();
    assert_eq!(summary.counts.accepted, 1);
    assert_eq!(summary.retired.final_confirmed, 1);
    assert!(!summary.incomplete());
}
