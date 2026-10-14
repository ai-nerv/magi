//! Running the tools program once and collecting what it printed: in a process group of its own,
//! with its input written on a thread, its output capped at a frame, and a wait that a cancel or
//! a deadline ends by killing the group and reaping it.

use crate::Cancel;
use crate::family::FRAME_CAP;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How often a running child is looked at.
const POLL: Duration = Duration::from_millis(20);

/// How long output may keep arriving after the program itself has exited.
const AFTER_EXIT: Duration = Duration::from_secs(2);

/// Run `command` with `input` on its standard input, and return what it wrote to standard output.
///
/// # Errors
/// When it cannot start, is cancelled, outlives `patience`, prints more than a frame holds, or
/// leaves something holding its output after it exits.
pub(super) fn output(
    mut command: Command,
    input: Option<Vec<u8>>,
    cancel: &dyn Cancel,
    patience: Duration,
) -> Result<Vec<u8>, String> {
    command
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|why| format!("could not be started: {why}"))?;
    if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let (said, heard) = std::sync::mpsc::channel();
    if let Some(mut out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut held = Vec::new();
            let read = (&mut out).take(FRAME_CAP as u64 + 1).read_to_end(&mut held);
            let _ = said.send(read.map(|_| held));
        });
    }

    let started = Instant::now();
    let mut read = None;
    loop {
        if read.is_none() {
            read = heard.try_recv().ok();
        }
        let over = matches!(&read, Some(Ok(held)) if held.len() > FRAME_CAP);
        match child.try_wait() {
            Ok(Some(_)) if !over => break,
            Ok(_) => {}
            Err(why) => {
                stop(&mut child);
                return Err(format!("could not be waited for: {why}"));
            }
        }
        let ended = if over {
            Some(format!(
                "printed more than the {FRAME_CAP} bytes a frame holds"
            ))
        } else if cancel.is_cancelled() {
            Some("was cancelled".to_owned())
        } else if started.elapsed() > patience {
            Some(format!("was still running after {}s", patience.as_secs()))
        } else {
            None
        };
        if let Some(why) = ended {
            stop(&mut child);
            return Err(why);
        }
        std::thread::sleep(POLL);
    }

    let held = match read {
        Some(read) => read,
        None => heard
            .recv_timeout(AFTER_EXIT)
            .map_err(|_| "exited and left something holding its output".to_owned())?,
    };
    held.map_err(|why| format!("could not be read: {why}"))
}

/// Kill the child's whole process group, then reap the child.
fn stop(child: &mut Child) {
    if let Some(group) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::Kill);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Uncancelled;

    struct After(Instant);
    impl Cancel for After {
        fn is_cancelled(&self) -> bool {
            Instant::now() >= self.0
        }
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    #[test]
    fn what_it_printed_comes_back_and_its_input_went_in() {
        let out = output(
            shell("cat"),
            Some(b"hello".to_vec()),
            &Uncancelled,
            POLL * 500,
        )
        .expect("ran");
        assert_eq!(out, b"hello");
    }

    #[test]
    fn a_cancel_stops_a_long_sleep_and_its_children() {
        let started = Instant::now();
        let cancel = After(Instant::now() + Duration::from_millis(200));
        let why = output(
            shell("sleep 1000 & sleep 1000; wait"),
            None,
            &cancel,
            Duration::from_secs(60),
        )
        .expect_err("cancelled");
        assert!(why.contains("cancelled"), "{why}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_deadline_stops_what_nobody_cancels() {
        let why = output(
            shell("sleep 1000"),
            None,
            &Uncancelled,
            Duration::from_millis(200),
        )
        .expect_err("stopped");
        assert!(why.contains("still running"), "{why}");
    }

    #[test]
    fn output_past_a_frame_is_refused() {
        let why = output(
            shell("head -c 17000000 /dev/zero"),
            None,
            &Uncancelled,
            Duration::from_secs(30),
        )
        .expect_err("too much");
        assert!(why.contains("frame"), "{why}");
    }
}
