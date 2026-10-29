use super::super::{self as supplier, door, waiting};
use crate::holding::Context;
use crate::{Cancel, Uncancelled};
use magi_model::scratch::Scratch;
use magi_proto::tooling::Call;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::{Duration, Instant};

struct After(Instant);
impl Cancel for After {
    fn is_cancelled(&self) -> bool {
        Instant::now() >= self.0
    }
}

fn isolated(name: &str, door: &str) -> bool {
    const CHILD: &str = "MAGI_NATIVE_ADMISSION_TEST";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return false;
    }
    let dir = Scratch::new("magi-file-admission", name);
    let result = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            &format!("supplier::admission::processes::{name}"),
            "--nocapture",
        ])
        .env(CHILD, name)
        .env(supplier::DOOR, door)
        .current_dir(&*dir)
        .output()
        .expect("isolated fixture");
    assert!(
        result.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    true
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("role source");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("role mode");
}

fn mutation() -> Call {
    serde_json::from_value(
        json!({"tool":"store","args":{"path":"source","contents":"after"},
        "file_revision":{"version":1,"operation":"write"},"revision_contract":1,
        "expected_revision":"absent","mutation_id":"logical-1"}),
    )
    .expect("independent guarded call")
}

#[test]
fn guarded_command_payload_waits_for_the_actual_process_and_old_or_partial_peers_receive_nothing() {
    let name = "guarded_command_payload_waits_for_the_actual_process_and_old_or_partial_peers_receive_nothing";
    if isolated(name, "command") {
        return;
    }
    let dir = Scratch::new("magi-file-admission", "command");
    let program = dir.join("role");
    let reader = supplier::fixture_reader();
    let started = dir.join("started");
    let input = dir.join("input");
    for announcement in [
        "old peer\n".to_owned(),
        "{\"event\":\"file-admission\",\"revision_contract\":2}\n".to_owned(),
        "{\"event\":\"file-admission\",\"revision_contract\":null}\n".to_owned(),
        "{\"event\":\"file-admission\",\"revision_contract\":1}".to_owned(),
        "x".repeat(1100),
    ] {
        script(
            &program,
            &format!(
                ": > '{}'; : > '{}'; printf %s '{}'; {reader} >> '{}'",
                started.display(),
                input.display(),
                announcement,
                input.display()
            ),
        );
        let why = supplier::run_jailed(
            program.to_str().expect("path"),
            &mutation(),
            "",
            None,
            &After(Instant::now() + Duration::from_millis(250)),
        )
        .expect_err("not admitted");
        assert!(started.exists(), "never reached actual process");
        assert!(
            why.contains("admission") || why.contains("cancelled"),
            "{why}"
        );
        assert_eq!(
            std::fs::read(&input).expect("input marker"),
            b"",
            "guarded bytes reached an old peer"
        );
    }
    let reply = json!({"ok":true,"n":1,"result":[{"said":"once"}]});
    script(
        &program,
        &format!(
            "printf '%s\\n' '{{\"event\":\"file-admission\",\"revision_contract\":1}}'; {reader} > '{}'; printf '%s\\n' '{reply}'",
            input.display()
        ),
    );
    let ran = supplier::run(program.to_str().expect("path"), &mutation()).expect("same process");
    assert_eq!(ran.said, "once");
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&input).expect("actual input"))
            .expect("call"),
        serde_json::to_value(mutation()).expect("expected")
    );
}

#[test]
fn replacing_a_role_after_positive_cards_cannot_bypass_command_admission_or_replay_a_lost_commit() {
    let name = "replacing_a_role_after_positive_cards_cannot_bypass_command_admission_or_replay_a_lost_commit";
    if isolated(name, "command") {
        return;
    }
    let dir = Scratch::new("magi-file-admission", "swap");
    let program = dir.join("role");
    let reader = supplier::fixture_reader();
    let card = json!({"ok":true,"n":1,"result":[{"name":"store","description":"native","parameters":{},
        "file_revision":{"version":1,"operation":"write"}}]});
    script(&program, &format!("printf '%s\\n' '{card}'"));
    assert!(
        supplier::cards_from(program.to_str().expect("path"))[0]
            .file_revision
            .is_some()
    );
    let input = dir.join("input");
    script(
        &program,
        &format!(
            ": > '{}'; {reader} >> '{}'",
            input.display(),
            input.display()
        ),
    );
    supplier::run_jailed(
        program.to_str().expect("path"),
        &mutation(),
        "",
        None,
        &After(Instant::now() + Duration::from_millis(250)),
    )
    .expect_err("replaced old role");
    assert!(
        std::fs::read(&input)
            .expect("actual old peer input")
            .is_empty()
    );
    let count = dir.join("committed");
    script(
        &program,
        &format!(
            "printf '%s\\n' '{{\"event\":\"file-admission\",\"revision_contract\":1}}'; {reader} > '{}'; printf x >> '{}'; printf 'lost-reply'",
            input.display(),
            count.display()
        ),
    );
    let why = supplier::run(program.to_str().expect("path"), &mutation()).expect_err("reply lost");
    assert!(why.contains("reply"), "{why}");
    assert_eq!(
        std::fs::read(&count).expect("commit marker"),
        b"x",
        "mutation replayed"
    );
}

struct Cached<'a> {
    program: &'a str,
    context: &'a Context,
}
impl Drop for Cached<'_> {
    fn drop(&mut self) {
        door::discard(self.program, self.context);
    }
}

fn daemon_role(program: &Path, admitted: bool, marker: &Path) {
    let card = json!({"ok":true,"n":1,"result":[{"name":"store","description":"native","parameters":{},
        "file_revision":{"version":1,"operation":"write"}}]});
    let capability = if admitted {
        ",\"file_admission\":1"
    } else {
        ""
    };
    script(
        program,
        &format!(
            "case \"$1\" in\n tools) printf '%s\\n' '{card}';;\n serve) read key; printf '{{\"event\":\"listening\",\"at\":\"%s\"{capability}}}\\n' \"$3\"; exec sleep 1000;;\n run) printf replay >> '{}'; printf '%s\\n' '{{\"event\":\"file-admission\",\"revision_contract\":1}}'; {} > /dev/null; printf '%s\\n' '{{\"ok\":true,\"result\":[{{\"said\":\"fallback\"}}]}}';;\n esac",
            marker.display(),
            supplier::fixture_reader()
        ),
    );
}

fn frame(stream: &mut std::os::unix::net::UnixStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bounded frame");
    let mut size = [0; 4];
    stream.read_exact(&mut size).expect("frame size");
    let mut bytes = vec![0; u32::from_be_bytes(size) as usize];
    stream.read_exact(&mut bytes).expect("frame body");
    serde_json::from_slice(&bytes).expect("request")
}

#[test]
fn fresh_cards_never_upgrade_an_old_cached_daemon_and_sent_native_calls_are_not_replayed() {
    let name =
        "fresh_cards_never_upgrade_an_old_cached_daemon_and_sent_native_calls_are_not_replayed";
    if isolated(name, "library") {
        return;
    }
    let dir = Scratch::new("magi-file-admission", "cached");
    let program = dir.join("role");
    let marker = dir.join("replayed");
    daemon_role(&program, false, &marker);
    let program = program.to_str().expect("path");
    let context = Context::default();
    let _cached = Cached {
        program,
        context: &context,
    };
    let (path, _, capability, _) = door::serving(program, &context).expect("old actual daemon");
    assert!(!capability);
    magi_ipc::private_dir(path.parent().expect("runtime parent"))
        .expect("fixture runtime directory");
    let listener = UnixListener::bind(&path).expect("old socket");
    listener.set_nonblocking(true).expect("nonblocking");
    daemon_role(Path::new(program), true, &marker);
    assert!(supplier::cards_from(program)[0].file_revision.is_some());
    let why = supplier::run_context(program, &mutation(), &context, &Uncancelled)
        .expect_err("old cached process");
    assert!(why.contains("selected tools daemon"), "{why}");
    assert!(matches!(listener.accept(), Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    assert!(!marker.exists(), "old cached peer fell back to a command");
    let newer = Context {
        configure: "new-admitted-daemon".into(),
        ..Context::default()
    };
    let _newer = Cached {
        program,
        context: &newer,
    };
    let (path, _, capability, _) = door::serving(program, &newer).expect("new actual daemon");
    assert!(capability);
    let listener = UnixListener::bind(&path).expect("admitted socket");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("guarded request");
        let request = frame(&mut stream);
        stream.write_all(&8_u32.to_be_bytes()).expect("reply size");
        stream.write_all(b"not JSON").expect("lost reply");
        request
    });
    supplier::run_context(program, &mutation(), &newer, &Uncancelled).expect_err("sent reply lost");
    let request = server.join().expect("server");
    assert_eq!(request["args"][0]["mutation_id"], "logical-1");
    assert!(
        request["args"][0]["args"]
            .get("expected_revision")
            .is_none()
    );
    assert!(!marker.exists(), "sent native operation was replayed");
    let unsent = Context {
        configure: "admitted-unsent".into(),
        ..Context::default()
    };
    let _unsent = Cached {
        program,
        context: &unsent,
    };
    assert!(door::serving(program, &unsent).expect("announced").2);
    let ran = supplier::run_context(program, &mutation(), &unsent, &Uncancelled)
        .expect("admitted command fallback");
    assert_eq!(ran.said, "fallback");
    assert_eq!(
        std::fs::read(&marker).expect("only unsent fallback"),
        b"replay"
    );
}

#[test]
fn cancelled_or_expired_command_admission_never_sends_the_withheld_payload() {
    let dir = Scratch::new("magi-file-admission", "cancelled");
    let input = dir.join("input");
    let mut command = std::process::Command::new("sh");
    command.args([
        "-c",
        &format!(
            ": > '{}'; {} >> '{}'",
            input.display(),
            supplier::fixture_reader(),
            input.display()
        ),
    ]);
    let why = waiting::admitted(
        command,
        b"guarded-payload".to_vec(),
        &After(Instant::now() + Duration::from_millis(250)),
        Duration::from_secs(5),
    )
    .expect_err("cancelled");
    assert!(why.contains("cancelled"), "{why}");
    assert!(std::fs::read(&input).expect("actual input").is_empty());
    let mut command = std::process::Command::new("sh");
    command.args([
        "-c",
        &format!(
            ": > '{}'; {} >> '{}'",
            input.display(),
            supplier::fixture_reader(),
            input.display()
        ),
    ]);
    let why = waiting::admitted(
        command,
        b"guarded-payload".to_vec(),
        &Uncancelled,
        Duration::from_millis(250),
    )
    .expect_err("expired");
    assert!(why.contains("still running"), "{why}");
    assert!(std::fs::read(&input).expect("actual input").is_empty());
    let program = dir.join("must-not-start");
    let started = dir.join("started");
    script(
        &program,
        &format!("printf started > '{}'", started.display()),
    );
    let why = supplier::run_jailed(
        program.to_str().expect("path"),
        &mutation(),
        "",
        None,
        &After(Instant::now()),
    )
    .expect_err("already cancelled");
    assert!(why.contains("cancelled before dispatch"), "{why}");
    assert!(!started.exists());
}
