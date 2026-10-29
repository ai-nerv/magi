use super::super::wire::{INPUT_MOST, REPLY_MOST};
use super::{Cancel, Error};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Owned(Child);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = rustix::process::kill_process_group(
            rustix::process::Pid::from_child(&self.0),
            rustix::process::Signal::Kill,
        );
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), Error> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|_| Error::Helper)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK).map_err(|_| Error::Helper)
}

pub(super) async fn exchange(
    mut command: Command,
    input: &[u8],
    cancel: &Cancel,
    deadline: Instant,
) -> Result<Vec<u8>, Error> {
    if input.len() > INPUT_MOST {
        return Err(Error::Protocol);
    }
    if cancel.is_requested() {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(Error::Timeout);
    }
    let mut child = Owned(
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| Error::Helper)?,
    );
    let mut stdin = child.0.stdin.take();
    let mut stdout = child.0.stdout.take();
    let mut stderr = child.0.stderr.take();
    nonblocking(stdin.as_ref().ok_or(Error::Helper)?)?;
    nonblocking(stdout.as_ref().ok_or(Error::Helper)?)?;
    nonblocking(stderr.as_ref().ok_or(Error::Helper)?)?;
    let mut sent = 0;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut status = None;
    loop {
        if cancel.is_requested() {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        if let Some(pipe) = &mut stdin {
            match pipe.write(input.get(sent..).ok_or(Error::Protocol)?) {
                Ok(0) if sent < input.len() => return Err(Error::Helper),
                Ok(written) => sent += written,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(Error::Helper),
            }
            if sent == input.len() {
                stdin = None;
            }
        }
        read_pipe(&mut stdout, &mut output, REPLY_MOST)?;
        read_pipe(&mut stderr, &mut errors, 4096)?;
        if status.is_none() {
            use rustix::process::{WaitId, WaitidOptions, waitid};
            status = waitid(
                WaitId::Pid(rustix::process::Pid::from_child(&child.0)),
                WaitidOptions::EXITED | WaitidOptions::NOHANG | WaitidOptions::NOWAIT,
            )
            .map_err(|_| Error::Helper)?
            .map(|status| status.exit_status() == Some(0));
        }
        if let Some(success) = status
            && stdout.is_none()
            && stderr.is_none()
        {
            if !success || sent != input.len() {
                return Err(Error::Helper);
            }
            return Ok(output);
        }
        tokio::select! {
            biased;
            _ = cancel.requested() => return Err(Error::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(1)) => {}
        }
    }
}

fn read_pipe<T: Read>(
    pipe: &mut Option<T>,
    output: &mut Vec<u8>,
    most: usize,
) -> Result<(), Error> {
    let Some(reader) = pipe else {
        return Ok(());
    };
    let mut buffer = [0; 8192];
    match reader.read(&mut buffer) {
        Ok(0) => *pipe = None,
        Ok(count) => {
            if count > most.saturating_sub(output.len()) {
                return Err(Error::Protocol);
            }
            output.extend_from_slice(buffer.get(..count).ok_or(Error::Protocol)?);
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) => {}
        Err(_) => return Err(Error::Helper),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
