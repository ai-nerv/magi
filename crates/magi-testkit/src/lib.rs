//! A harness that isn't one: a recorded event stream served over a real Unix socket. The transport
//! is production code; only the source of events is fake.

pub mod conformance;
pub mod mind;
pub mod replay;

pub use mind::Mind;

/// A temporary directory that removes itself. See [`magi_model::scratch`].
pub use magi_model::scratch::Scratch;
pub use replay::{FakeHarness, Recording};

/// Take away the variables that would point a spawned magi at somebody else's memory layer:
/// `MAGI_API_SOCKET` makes `balthasar::start` answer `Theirs`, so a test records into the
/// developer's own memory, and the instance variables move the socket directory. Both names of
/// that one: a developer's shell may still be setting either.
pub fn only_its_own_store(command: &mut std::process::Command) {
    command.env_remove("MAGI_API_SOCKET");
    command.env_remove("MAGI_MEMORY_INSTANCE");
    command.env_remove("MAGI_BALTHASAR_INSTANCE");
}

/// The first line a spawned process prints, or nothing if it has not printed one in `patience`.
/// `read_line` on a child's stdout has no deadline, so a session that never announces itself
/// blocks for ever. The reader keeps going after the first line, discarding: a child whose pipe
/// fills up stops rather than exits.
pub fn first_line_within(
    process: &mut std::process::Child,
    patience: std::time::Duration,
) -> Option<String> {
    use std::io::BufRead;
    let out = process.stdout.take()?;
    let (say, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(out);
        let mut line = String::new();
        let read = reader.read_line(&mut line).unwrap_or(0);
        let _ = say.send(if read > 0 { Some(line) } else { None });
        std::io::copy(&mut reader, &mut std::io::sink()).ok();
    });
    heard.recv_timeout(patience).ok().flatten()
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn saying(script: &str) -> std::process::Child {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh runs")
    }

    #[test]
    fn the_first_line_comes_back() {
        let mut child = saying("echo p/main/abc; sleep 0.2");
        let said = super::first_line_within(&mut child, Duration::from_secs(10));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(said.as_deref().map(str::trim), Some("p/main/abc"));
    }

    #[test]
    fn a_process_that_never_says_anything_is_given_up_on() {
        // Timed rather than merely asserted: a broken deadline gives the same `None` in the end.
        // `exec`, so the `kill` below reaches the sleep rather than the shell that forked it.
        let mut child = saying("exec sleep 30");
        let started = Instant::now();
        let said = super::first_line_within(&mut child, Duration::from_millis(300));
        let took = started.elapsed();
        let _ = child.kill();
        let _ = child.wait();
        assert!(said.is_none(), "it said {said:?}");
        assert!(took < Duration::from_secs(5), "it waited {took:?}");
    }

    #[test]
    fn a_process_that_says_nothing_and_exits_is_not_a_line() {
        let mut child = saying("exit 0");
        let said = super::first_line_within(&mut child, Duration::from_secs(10));
        let _ = child.wait();
        assert!(said.is_none(), "it said {said:?}");
    }
}
