use super::*;
use magi_testkit::Scratch;

fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    command
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn pid(path: &std::path::Path) -> Option<rustix::process::Pid> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()
        .and_then(rustix::process::Pid::from_raw)
}

async fn started(path: &std::path::Path) {
    let until = deadline();
    while pid(path).is_none() {
        assert!(Instant::now() < until, "worker did not start");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn reaped(path: &std::path::Path) {
    let pid = pid(path).expect("worker pid");
    assert!(matches!(
        rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG),
        Err(rustix::io::Errno::CHILD)
    ));
    assert!(
        !std::path::Path::new(&format!(
            "/proc/{}",
            rustix::process::Pid::as_raw(Some(pid))
        ))
        .exists()
    );
}

async fn not_running(path: &std::path::Path) {
    let pid = pid(path).expect("descendant pid");
    let until = deadline();
    loop {
        match std::fs::read_to_string(format!(
            "/proc/{}/stat",
            rustix::process::Pid::as_raw(Some(pid))
        )) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Ok(text)
                if text
                    .split_once(") ")
                    .is_some_and(|(_, state)| state.starts_with('Z')) =>
            {
                return;
            }
            _ => assert!(Instant::now() < until, "descendant still running"),
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test]
async fn exchanges_full_input_without_detached_pipe_threads() {
    let input = vec![b'x'; INPUT_MOST];
    assert_eq!(
        exchange(shell("cat"), &input, &Cancel::default(), deadline())
            .await
            .expect("echo"),
        input
    );
}

#[tokio::test]
async fn cancellation_deadline_and_input_limit_prevent_spawn() {
    let fixture = Scratch::new("skills-client", "no-spawn");
    let command = || {
        let mut command = shell("touch \"$MARKER\"");
        command.env("MARKER", fixture.join("started"));
        command
    };
    let cancel = Cancel::default();
    cancel.request();
    assert_eq!(
        exchange(command(), b"", &cancel, deadline()).await,
        Err(Error::Cancelled)
    );
    assert_eq!(
        exchange(command(), b"", &Cancel::default(), Instant::now()).await,
        Err(Error::Timeout)
    );
    assert_eq!(
        exchange(
            command(),
            &vec![0; INPUT_MOST + 1],
            &Cancel::default(),
            deadline()
        )
        .await,
        Err(Error::Protocol)
    );
    assert!(!fixture.join("started").exists());
}

#[tokio::test]
async fn each_pipe_is_bounded_and_failed_exit_is_rejected() {
    for script in ["head -c 8388609 /dev/zero", "head -c 4097 /dev/zero >&2"] {
        assert_eq!(
            exchange(shell(script), b"", &Cancel::default(), deadline()).await,
            Err(Error::Protocol)
        );
    }
    assert_eq!(
        exchange(
            shell("head -c 4096 /dev/zero >&2; printf ok"),
            b"",
            &Cancel::default(),
            deadline()
        )
        .await,
        Ok(b"ok".to_vec())
    );
    assert_eq!(
        exchange(shell("exit 7"), b"", &Cancel::default(), deadline()).await,
        Err(Error::Helper)
    );
}

#[tokio::test]
async fn deadline_covers_stalled_input_and_closed_output_without_exit() {
    for script in [
        "echo $$ > \"$PID\"; exec sleep 30",
        "echo $$ > \"$PID\"; exec 1>&- 2>&-; exec sleep 30",
    ] {
        let fixture = Scratch::new("skills-client", "timeout");
        let mut command = shell(script);
        command.env("PID", fixture.join("pid"));
        let began = Instant::now();
        assert_eq!(
            exchange(
                command,
                &vec![b'x'; INPUT_MOST],
                &Cancel::default(),
                began + Duration::from_secs(1)
            )
            .await,
            Err(Error::Timeout)
        );
        assert!(began.elapsed() < Duration::from_secs(3));
        reaped(&fixture.join("pid"));
    }
}

#[tokio::test]
async fn cancellation_and_dropping_the_future_both_reap_the_helper() {
    for abort in [false, true] {
        let fixture = Scratch::new("skills-client", "cancel");
        let mut command = shell("echo $$ > \"$PID\"; exec sleep 30");
        command.env("PID", fixture.join("pid"));
        let cancel = Cancel::default();
        let flag = cancel.clone();
        let task = tokio::spawn(async move { exchange(command, b"", &flag, deadline()).await });
        started(&fixture.join("pid")).await;
        if abort {
            task.abort();
            assert!(task.await.expect_err("aborted").is_cancelled());
        } else {
            cancel.request();
            assert_eq!(task.await.expect("joined"), Err(Error::Cancelled));
        }
        reaped(&fixture.join("pid"));
    }
}

#[tokio::test]
async fn exited_leader_remains_owned_until_descendant_cleanup() {
    for (script, expected) in [
        (
            "echo $$ > \"$PID\"; sleep 30 & echo $! > \"$DESC\"; exit 0",
            Err(Error::Timeout),
        ),
        (
            "echo $$ > \"$PID\"; sleep 30 1>&- 2>&- & echo $! > \"$DESC\"; printf ok; exit 0",
            Ok(b"ok".to_vec()),
        ),
    ] {
        let fixture = Scratch::new("skills-client", "descendants");
        let mut command = shell(script);
        command
            .env("PID", fixture.join("pid"))
            .env("DESC", fixture.join("descendant"));
        assert_eq!(
            exchange(
                command,
                b"",
                &Cancel::default(),
                Instant::now() + Duration::from_secs(1)
            )
            .await,
            expected
        );
        reaped(&fixture.join("pid"));
        not_running(&fixture.join("descendant")).await;
    }
}
