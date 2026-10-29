//! Running the tools program once and collecting what it printed: in a process group of its own,
//! with its input written on a thread, its output capped at a frame, and a wait that a cancel or
//! a deadline ends by killing the group and reaping it.

use super::failure::Failure;
use crate::Cancel;
use crate::family::FRAME_CAP;
use crate::native::{Child, Input};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
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
    command: Command,
    input: Option<Vec<u8>>,
    cancel: &dyn Cancel,
    patience: Duration,
) -> Result<Vec<u8>, String> {
    output_known(command, input, cancel, patience).map_err(Failure::into_string)
}

#[cfg(test)]
pub(super) fn admitted(
    command: Command,
    input: Vec<u8>,
    cancel: &dyn Cancel,
    patience: Duration,
) -> Result<Vec<u8>, String> {
    admitted_known(command, input, cancel, patience).map_err(Failure::into_string)
}

pub(super) fn output_known(
    command: Command,
    input: Option<Vec<u8>>,
    cancel: &dyn Cancel,
    patience: Duration,
) -> Result<Vec<u8>, Failure> {
    collect(command, input, cancel, patience, false, false)
}

#[cfg(test)]
pub(super) fn admitted_known(
    command: Command,
    input: Vec<u8>,
    cancel: &dyn Cancel,
    patience: Duration,
) -> Result<Vec<u8>, Failure> {
    collect(command, Some(input), cancel, patience, true, false)
}

pub(super) fn run_known(
    command: Command,
    input: Vec<u8>,
    cancel: &dyn Cancel,
    patience: Duration,
    admission: bool,
) -> Result<Vec<u8>, Failure> {
    collect(command, Some(input), cancel, patience, admission, true)
}

fn collect(
    mut command: Command,
    input: Option<Vec<u8>>,
    cancel: &dyn Cancel,
    patience: Duration,
    admission: bool,
    observed: bool,
) -> Result<Vec<u8>, Failure> {
    if cancel.is_cancelled() {
        return Err(Failure::refused("was cancelled"));
    }
    command
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let handoff = observed
        .then(|| super::launch::prepare(&command, cancel, super::launch::Kind::Call))
        .flatten();
    if cancel.is_cancelled() {
        return Err(Failure::refused("was cancelled before dispatch"));
    }
    let mut child = crate::native::spawn(command, handoff)
        .map_err(|why| Failure::refused(format!("could not be started: {why}")))?;
    let mut sent = false;
    let mut pending = child.input.take().zip(input);
    if !admission {
        sent |= send(&mut pending);
    }
    let (said, heard) = std::sync::mpsc::channel();
    let (announced, announcement) = std::sync::mpsc::channel();
    if let Some(mut out) = child.stdout.take() {
        std::thread::spawn(move || {
            if admission {
                let event = super::admission::command(&mut out);
                let refused = event.is_err();
                let _ = announced.send(event);
                if refused {
                    return;
                }
            }
            let mut held = Vec::new();
            let read = (&mut out).take(FRAME_CAP as u64 + 1).read_to_end(&mut held);
            let _ = said.send(read.map(|_| held));
        });
    }

    let started = Instant::now();
    let mut read = None;
    loop {
        let ended = if cancel.is_cancelled() {
            Some("was cancelled".to_owned())
        } else if started.elapsed() > patience {
            Some(format!("was still running after {}s", patience.as_secs()))
        } else if pending.is_some() && started.elapsed() > super::admission::PATIENCE {
            Some("native file admission timed out before any request bytes were sent".to_owned())
        } else {
            None
        };
        if let Some(why) = ended {
            stop(&mut child);
            return Err(Failure::after(sent, why));
        }
        if pending.is_some() {
            match announcement.try_recv() {
                Ok(Ok(())) => {
                    sent |= send(&mut pending);
                }
                Ok(Err(why)) => {
                    stop(&mut child);
                    return Err(Failure::after(sent, why));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    stop(&mut child);
                    return Err(Failure::refused(
                        "native file admission ended before a supported announcement; request not sent",
                    ));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if read.is_none() {
            read = heard.try_recv().ok();
        }
        let over = matches!(&read, Some(Ok(held)) if held.len() > FRAME_CAP);
        match child.try_wait() {
            Ok(Some(_)) if !over => break,
            Ok(_) => {}
            Err(why) => {
                stop(&mut child);
                return Err(Failure::after(
                    sent,
                    format!("could not be waited for: {why}"),
                ));
            }
        }
        let ended = if over {
            Some(format!(
                "printed more than the {FRAME_CAP} bytes a frame holds"
            ))
        } else if let Some(Err(why)) = &read {
            Some(format!("could not be read: {why}"))
        } else {
            None
        };
        if let Some(why) = ended {
            stop(&mut child);
            return Err(Failure::after(sent, why));
        }
        std::thread::sleep(POLL);
    }

    if pending.is_some() {
        stop(&mut child);
        return Err(Failure::refused(
            "native file admission exited without admitting the request; request not sent",
        ));
    }
    let held = match read {
        Some(read) => read,
        None => match heard.recv_timeout(AFTER_EXIT) {
            Ok(read) => read,
            Err(_) => {
                stop(&mut child);
                return Err(Failure::after(
                    sent,
                    "exited and left something holding its output",
                ));
            }
        },
    };
    held.map_err(|why| Failure::after(sent, format!("could not be read: {why}")))
}

fn send(pending: &mut Option<(Input, Vec<u8>)>) -> bool {
    if let Some((mut stdin, input)) = pending.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        true
    } else {
        false
    }
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

    #[test]
    fn refusal_before_payload_and_uncertainty_after_handoff_follow_actual_send_state() {
        let failure = admitted_known(
            shell("printf 'old peer\\n'"),
            b"guarded".to_vec(),
            &Uncancelled,
            Duration::from_secs(3),
        )
        .expect_err("no admission");
        assert!(failure.no_effect());
        let dir = magi_model::scratch::Scratch::new("magi-native-send", "state");
        let marker = dir.join("sent");
        struct Seen(std::path::PathBuf);
        impl Cancel for Seen {
            fn is_cancelled(&self) -> bool {
                self.0.exists()
            }
        }
        let failure=admitted_known(shell(&format!(
            "printf '%s\\n' '{{\"event\":\"file-admission\",\"revision_contract\":1}}'; cat > '{}'; touch '{}'; exec sleep 1000",
            dir.join("input").display(),marker.display())),b"guarded".to_vec(),&Seen(marker),Duration::from_secs(5)).expect_err("cancelled after send");
        assert!(!failure.no_effect());
        assert_eq!(
            std::fs::read(dir.join("input")).expect("payload reached peer"),
            b"guarded"
        );
    }
}
