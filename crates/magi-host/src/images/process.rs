use super::{Reply, Request, WIRE_BYTES, WORKER_ARG};
use magi_model::Image;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

static WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

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

pub(super) async fn run(request: Request) -> Result<Vec<Image>, String> {
    if request.images.is_empty() {
        return Ok(Vec::new());
    }
    magi_model::validate_images(&request.images)?;
    let mut command = Command::new("/proc/self/exe");
    command.arg(WORKER_ARG);
    run_with(command, request, Duration::from_secs(8)).await
}

async fn run_with(
    mut command: Command,
    request: Request,
    patience: Duration,
) -> Result<Vec<Image>, String> {
    let deadline = tokio::time::Instant::now() + patience;
    let _permit = tokio::time::timeout_at(deadline, WORKERS.acquire())
        .await
        .map_err(|_| "Image normalization timed out.")?
        .map_err(|_| "Image worker is unavailable.")?;
    let expected = request.images.len();
    let input =
        serde_json::to_vec(&request).map_err(|_| "Could not encode image-worker request.")?;
    if input.len() > WIRE_BYTES {
        return Err("Image-worker request exceeds its byte limit.".into());
    }
    command
        .env_clear()
        .current_dir("/")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = Owned(
        command
            .spawn()
            .map_err(|_| "Could not start image worker.")?,
    );
    let mut stdin = Some(
        child
            .0
            .stdin
            .take()
            .ok_or("Image worker has no input pipe.")?,
    );
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or("Image worker has no output pipe.")?;
    nonblocking(stdin.as_ref().ok_or("Image worker has no input pipe.")?)?;
    nonblocking(&stdout)?;
    let mut written = 0;
    let mut output = Vec::new();
    let mut eof = false;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("Image normalization timed out.".into());
        }
        if let Some(pipe) = &mut stdin {
            match pipe.write(input.get(written..).ok_or("Invalid image input offset.")?) {
                Ok(0) => return Err("Image worker closed its input.".into()),
                Ok(n) => {
                    written += n;
                }
                Err(error) if waiting(&error) => {}
                Err(_) => return Err("Could not send image-worker request.".into()),
            }
            if written == input.len() {
                stdin.take();
            }
        }
        if !eof {
            let mut buffer = [0; 8192];
            match stdout.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(n) => {
                    if n > WIRE_BYTES.saturating_sub(output.len()) {
                        return Err("Image-worker reply exceeds its byte limit.".into());
                    }
                    output
                        .extend_from_slice(buffer.get(..n).ok_or("Invalid image output length.")?);
                }
                Err(error) if waiting(&error) => {}
                Err(_) => return Err("Could not read image-worker reply.".into()),
            }
        }
        let status = rustix::process::waitid(
            rustix::process::WaitId::Pid(rustix::process::Pid::from_child(&child.0)),
            rustix::process::WaitidOptions::EXITED
                | rustix::process::WaitidOptions::NOHANG
                | rustix::process::WaitidOptions::NOWAIT,
        )
        .map_err(|_| "Could not inspect image worker.")?;
        if let Some(status) = status
            && eof
        {
            if status.exit_status() != Some(0) {
                return Err("Image worker stopped without a valid result.".into());
            }
            return reply(&output, expected);
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn waiting(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}

fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), String> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|_| "Could not inspect image-worker pipe.")?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| "Could not configure image-worker pipe.".into())
}

fn reply(bytes: &[u8], count: usize) -> Result<Vec<Image>, String> {
    match serde_json::from_slice::<Reply>(bytes).map_err(|_| "Invalid image-worker reply.")? {
        Reply::Ok { version: 1, images } if images.len() == count => {
            magi_model::validate_images(&images)?;
            Ok(images)
        }
        Reply::Failed { version: 1, error } => Err(error.message().into()),
        _ => Err("Invalid image-worker reply version or image count.".into()),
    }
}

#[cfg(test)]
mod tests;
