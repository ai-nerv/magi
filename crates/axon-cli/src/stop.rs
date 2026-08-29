//! `axon stop` — ending a daemon on purpose.
//!
//! A UI quitting is a detach: the turn keeps running, which is the point of the daemon owning
//! the session. The cost is that nothing ever ends one, and a week of work leaves a process
//! per project still holding a socket. `ps | grep axon` is not an answer a tool should expect
//! of the person using it.
//!
//! By recorded pid, never by matching command lines. A pattern is compared against every
//! process on the machine, and a working directory is a prefix that other things share — which
//! is how a careless `pkill -f` takes down the terminal you ran it from.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a daemon is given to close its socket before it is reported as stuck.
const PATIENCE: Duration = Duration::from_secs(5);

/// How often the socket is retried while waiting.
const POLL: Duration = Duration::from_millis(50);

/// Stop the daemon for `socket`, or every one this user has running.
pub fn run(socket: &Path, all: bool) -> Result<()> {
    let targets = if all {
        running()
    } else {
        vec![crate::daemon::pid_path(socket)]
    };

    let mut stopped = 0;
    for pid_file in targets {
        match stop_one(&pid_file) {
            Stopped::Yes => stopped += 1,
            Stopped::NotRunning => {}
            Stopped::Stuck(pid) => {
                eprintln!("axon: {pid} did not stop; it may be mid-turn");
            }
        }
    }

    match stopped {
        0 => println!("No daemon was running."),
        1 => println!("Stopped 1 daemon."),
        n => println!("Stopped {n} daemons."),
    }
    Ok(())
}

/// What happened to one daemon.
pub(crate) enum Stopped {
    /// It is gone.
    Yes,
    /// There was nothing there, which is not a failure.
    NotRunning,
    /// It was asked and did not go.
    Stuck(u32),
}

/// Ask one daemon to stop, and wait for its socket to close.
pub(crate) fn stop_one(pid_file: &Path) -> Stopped {
    let Some(pid) = std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
    else {
        return Stopped::NotRunning;
    };
    let socket = pid_file.with_extension("sock");

    let Some(target) = rustix::process::Pid::from_raw(pid.cast_signed()) else {
        return Stopped::NotRunning;
    };
    // `TERM`, not `KILL`: the daemon closes its socket, removes its files and exits, and a
    // journal is left on a record boundary rather than wherever the write had got to.
    if rustix::process::kill_process(target, rustix::process::Signal::Term).is_err() {
        // Already gone. The pid file outlived it, which is what a crash leaves behind.
        let _ = std::fs::remove_file(pid_file);
        return Stopped::NotRunning;
    }

    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            let _ = std::fs::remove_file(pid_file);
            return Stopped::Yes;
        }
        std::thread::sleep(POLL);
    }
    Stopped::Stuck(pid)
}

/// Every daemon this user has, by its pid file.
fn running() -> Vec<PathBuf> {
    let Some(dir) = axon_ipc::default_socket_path()
        .parent()
        .map(Path::to_path_buf)
    else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "pid"))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pid_file_with_nothing_in_it_is_not_running() {
        let dir = std::env::temp_dir().join(format!("axon-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("host.pid");
        std::fs::write(&path, "not a number").expect("write");
        assert!(matches!(stop_one(&path), Stopped::NotRunning));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pid_file_for_a_process_that_is_gone_is_cleaned_up() {
        // What a crash leaves: the file outlives the process. Reporting it as stuck would send
        // people looking for something that is not there.
        let dir = std::env::temp_dir().join(format!("axon-stop-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("host.pid");
        // A pid that cannot be running: one past the maximum any Linux will assign.
        std::fs::write(&path, "4194305").expect("write");
        assert!(matches!(stop_one(&path), Stopped::NotRunning));
        assert!(!path.exists(), "the stale file is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_pid_file_is_not_an_error() {
        let path = std::env::temp_dir().join("axon-stop-nothing-here.pid");
        let _ = std::fs::remove_file(&path);
        assert!(matches!(stop_one(&path), Stopped::NotRunning));
    }
}
