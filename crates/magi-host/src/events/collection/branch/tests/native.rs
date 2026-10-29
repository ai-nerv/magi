use super::*;
use crate::events::{
    Loss,
    collection::{Report, Sent},
};
use rustix::fd::AsRawFd;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const MODE: &str = "MAGI_TEST_BRANCH_STAGE";
const TARGETS: &str = "MAGI_TEST_BRANCH_FDS";
const INPUT: &str = "MAGI_TEST_BRANCH_INPUT";
const IMAGE: &str = "MAGI_TEST_BRANCH_IMAGE";
const CASE: &str = "events::collection::branch::tests::native::native_two_hop_branch_restores_input_and_excludes_all_observation_descriptors";
const PAYLOAD: &[u8] = b"branch input is ordinary input, not event metadata";

fn target(fd: impl AsFd) -> PathBuf {
    std::fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd())).expect("fd identity")
}

fn command(stage: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("test image"));
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
        if let Some(status) = child.0.try_wait().expect("wait") {
            assert!(status.success(), "branch native child failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "branch native deadline");
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

fn probe(targets: &str) {
    let mut unrelated = command("probe");
    unrelated.env(TARGETS, targets).stdin(Stdio::inherit());
    joined(unrelated);
}

#[test]
fn native_two_hop_branch_restores_input_and_excludes_all_observation_descriptors() {
    let stage = std::env::var(MODE).ok();
    if stage.is_some() {
        assert_eq!(
            std::fs::read_link("/proc/self/exe").expect("actual image"),
            PathBuf::from(std::env::var_os(IMAGE).expect("selected image"))
        );
    }
    match stage.as_deref() {
        Some("probe") => {
            let targets: Vec<_> = std::env::var(TARGETS)
                .expect("targets")
                .split(',')
                .map(PathBuf::from)
                .collect();
            for entry in std::fs::read_dir("/proc/self/fd").expect("descriptors") {
                if let Ok(link) = std::fs::read_link(entry.expect("entry").path()) {
                    assert!(
                        !targets.contains(&link),
                        "event/control/bootstrap descriptor inherited: {link:?}"
                    );
                }
            }
            assert_eq!(
                target(std::io::stdin()),
                PathBuf::from(std::env::var_os(INPUT).expect("stdin identity"))
            );
            return;
        }
        Some(stage @ ("helper" | "child")) => {
            let branch = Branch::receive_stdin().expect("immediate-parent bootstrap");
            assert_eq!(
                target(std::io::stdin()),
                PathBuf::from(std::env::var_os(INPUT).expect("stdin identity"))
            );
            let targets = std::env::var(TARGETS).expect("observation identities");
            probe(&targets);
            if stage == "helper" {
                let packet = branch
                    .prepare(std::io::stdin())
                    .expect("fresh child packet");
                let targets = format!("{targets},{}", target(&packet).display());
                probe(&targets);
                let mut child = command("child");
                child.env(TARGETS, targets).stdin(Stdio::from(packet));
                joined(child);
            } else {
                let mut input = Vec::new();
                std::io::stdin()
                    .take(129)
                    .read_to_end(&mut input)
                    .expect("ordinary stdin");
                assert_eq!(input, PAYLOAD);
                let (mut sender, _control, parent) = branch.into_parts();
                assert_eq!(parent.turn_id, Some(id(4)));
                assert_eq!(parent.ask_id, Some(id(5)));
                let bytes = serde_json::to_vec(&serde_json::json!({
                    "schema":1,"program":"magi","program_version":[1,0,0],
                    "producer":sender.identity(),"sequence":1,"correlation":sender.lineage(),
                    "fact":{"kind":"agent_started","parent":parent}
                }))
                .expect("event");
                assert_eq!(sender.send(&bytes), Sent::Queued);
                assert_eq!(
                    sender.report(Loss::default(), true),
                    Report::Sent(Sent::Queued)
                );
            }
            return;
        }
        None => {}
        _ => panic!("unknown branch fixture stage"),
    }
    let (worker, mut host, branch) = setup();
    let (sender, control, parent) = branch.into_parts();
    let lineage = sender.lineage();
    let (inner, owner, producer) = sender.into_fresh().expect("fresh sender");
    let event = inner.into_socket();
    let identities = format!(
        "{},{}",
        target(&event).display(),
        target(&control).display()
    );
    let sender = Sender::new(
        transport::Sender::adopt(event).expect("event sender"),
        owner,
        producer,
    )
    .with_lineage(lineage);
    let branch = Branch::new(sender, control, parent).expect("branch");
    let (input, mut writer) = UnixStream::pair().expect("ordinary input");
    writer.write_all(PAYLOAD).expect("input body");
    writer
        .shutdown(std::net::Shutdown::Write)
        .expect("input EOF");
    let input_identity = target(&input);
    let packet = branch.prepare(input).expect("helper packet");
    let identities = format!("{identities},{}", target(&packet).display());
    let mut helper = command("helper");
    helper
        .env(TARGETS, identities)
        .env(INPUT, input_identity)
        .env(IMAGE, std::env::current_exe().expect("selected executable"))
        .stdin(Stdio::from(packet));
    joined(helper);
    assert!(!host.poll());
    let deadline = Instant::now() + Duration::from_secs(3);
    while worker.readout().snapshot().retired.final_confirmed != 1 {
        assert!(
            Instant::now() < deadline,
            "native child retirement deadline"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.counts.accepted, 1);
    assert_eq!(summary.counts.invalid, 0);
    assert_eq!(summary.retired.final_confirmed, 1);
    assert!(!summary.incomplete());
}
