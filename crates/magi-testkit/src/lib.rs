//! A harness that isn't one.
//!
//! Serves a recorded event stream over a real Unix socket so the UI can be developed against
//! a file instead of a model. The transport is production code; only the source of events is
//! fake, which is what makes this useful rather than a mock.

pub mod conformance;
pub mod mind;
pub mod replay;

pub use mind::Mind;

/// A temporary directory that removes itself, even when a test panics. See [`magi_model::scratch`].
pub use magi_model::scratch::Scratch;
pub use replay::{FakeHarness, Recording};

/// Take away the two variables that would point a spawned magi at somebody else's balthasar.
///
/// **The suite is developed from inside a magi session, and a session exports these.**
/// `MAGI_API_SOCKET` set means "somebody already said which balthasar to talk to", so
/// `balthasar::start` answers `Theirs` and convenes none — the test's magi then records its
/// prompts into the *developer's own memory* and resumes out of it. Both halves of that are bad:
/// the run writes where it was never meant to, and `resume_live` then asserts about a store four
/// other things are also writing to, so it fails for a reason that has nothing to do with magi.
/// `MAGI_BALTHASAR_INSTANCE` does the quieter version of the same thing by moving the socket
/// directory out from under the `XDG_RUNTIME_DIR` each test carefully set.
///
/// `forking.rs` already removes `MAGI_API_SOCKET` when it starts a child, and says why at length.
/// This is the same removal for the same reason, on the way into a test rather than a fork.
pub fn only_its_own_store(command: &mut std::process::Command) {
    command.env_remove("MAGI_API_SOCKET");
    command.env_remove("MAGI_BALTHASAR_INSTANCE");
}

/// The first line a spawned process prints, or nothing if it has not printed one in `patience`.
///
/// **`read_line` on a child's stdout has no deadline, and that is how a suite hangs instead of
/// failing.** Both live tests that start a session wait for its name this way, and a session that
/// comes up and never announces itself — one whose melchior was killed out from under it, which
/// happened while this was being written — leaves the read blocked for ever. `cargo test` waits,
/// `gate-hermetic` waits, and CI waits until the job's own timeout kills it with nothing to say.
/// Twelve minutes went into one of those before anybody thought to look at `ps`.
///
/// The deadline is a backstop and not an assertion, so it is set well above anything a healthy
/// session takes: it can only fire where the alternative was waiting for ever.
///
/// The reader keeps going after the first line, discarding. Nothing reads a session's stdout
/// again, and a child whose pipe fills up stops rather than exits — a second way to wait for
/// ever, and one this arrangement closes on the way past.
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
        // The thread ends when the child does, because that is what closes the pipe.
        std::io::copy(&mut reader, &mut std::io::sink()).ok();
    });
    heard.recv_timeout(patience).ok().flatten()
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Start `sh -c` with its stdout on a pipe.
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
        // **The whole point, and it has to be timed rather than merely asserted.** A broken
        // deadline gives the same `None` in the end — it just takes the child's lifetime to do
        // it, which in the live suites is for ever. So the clock is what is checked.
        // `exec`, so `kill` below reaches the sleep. Without it `sh` forks one and the kill takes
        // only the shell: the sleep is reparented to init and outlives the whole suite, which is
        // what `gate-hermetic` found here once it started asking by environment as well as by
        // working directory.
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
