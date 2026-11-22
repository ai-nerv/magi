use magi_proto::authentication::{Reply, Request};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const LIMIT: usize = 512 * 1024;

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

pub(super) async fn run(
    program: &str,
    environ: &BTreeMap<String, String>,
    action: Request,
    request: u64,
    to: mpsc::Sender<magi_proto::HarnessEvent>,
) -> bool {
    let changed = !matches!(action, Request::List);
    let result = exchange(program, environ, action, request, &to).await;
    let (reply, success) = match result {
        Ok(reply) => {
            let success = matches!(&reply, Reply::Finished { error: None, .. });
            (reply, success)
        }
        Err(message) => (Reply::failed(message), false),
    };
    let _ = to
        .send(magi_proto::HarnessEvent::Authentication { request, reply })
        .await;
    changed && success
}

async fn exchange(
    program: &str,
    environ: &BTreeMap<String, String>,
    action: Request,
    request: u64,
    to: &mpsc::Sender<magi_proto::HarnessEvent>,
) -> Result<Reply, &'static str> {
    let patience = if matches!(action, Request::Login { .. }) {
        960
    } else {
        30
    };
    let mut value = serde_json::to_value(action).map_err(|_| "Invalid authentication request")?;
    value
        .as_object_mut()
        .ok_or("Invalid authentication request")?
        .insert("version".into(), 1.into());
    let mut input = serde_json::to_vec(&value).map_err(|_| "Invalid authentication request")?;
    input.push(b'\n');
    if input.len() > 32 * 1024 {
        return Err("Authentication request exceeds limit");
    }
    let mut child = Owned(
        Command::new(program)
            .envs(environ)
            .args(["auth", "--stdio"])
            .env(crate::driving::GIVEN, crate::driving::given_dir())
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Could not start Melchior authentication")?,
    );
    let mut stdin = child.0.stdin.take();
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or("Missing authentication output")?;
    if let Some(stdin) = &stdin {
        nonblocking(stdin)?;
    }
    nonblocking(&stdout)?;
    let deadline = Instant::now() + Duration::from_secs(patience);
    let mut written = 0;
    let mut total = 0;
    let mut line = Vec::new();
    let mut result = None;
    let mut browser = false;
    let mut eof = false;
    loop {
        if Instant::now() >= deadline {
            return Err("Authentication timed out; reopen :login to check saved status");
        }
        if let Some(pipe) = stdin.as_mut() {
            match pipe.write(
                input
                    .get(written..)
                    .ok_or("Invalid authentication input length")?,
            ) {
                Ok(n) => written += n,
                Err(e) if transient(&e) => {}
                Err(_) => {
                    return Err("Melchior refused authentication; update the model-role binary");
                }
            }
            if written == input.len() {
                stdin = None;
                input.fill(0);
                input.clear();
            }
        }
        if !eof {
            let mut bytes = [0; 8192];
            match stdout.read(&mut bytes) {
                Ok(0) => eof = true,
                Ok(n) => {
                    total += n;
                    if total > LIMIT {
                        return Err("Authentication reply exceeds limit");
                    }
                    for byte in bytes.iter().take(n) {
                        if *byte != b'\n' {
                            line.push(*byte);
                            continue;
                        }
                        let reply: Reply = serde_json::from_slice(&line)
                            .map_err(|_| "Invalid authentication reply; update Melchior")?;
                        line.clear();
                        if result.is_some() {
                            return Err("Unexpected authentication reply");
                        }
                        match &reply {
                            Reply::Device {
                                url,
                                code,
                                expires_in,
                            } => {
                                if browser
                                    || url.len() > 16384
                                    || !safe_browser_url(url)
                                    || code.is_empty()
                                    || code.len() > 32
                                    || !code.bytes().all(|b| b.is_ascii_graphic())
                                    || *expires_in == 0
                                    || *expires_in > 900
                                {
                                    return Err("Invalid device sign-in instructions");
                                }
                                browser = true;
                                to.send(magi_proto::HarnessEvent::Authentication {
                                    request,
                                    reply,
                                })
                                .await
                                .map_err(|_| "Authentication cancelled")?;
                            }
                            Reply::Browser { url } => {
                                if browser || url.len() > 16384 || !safe_browser_url(url) {
                                    return Err("Invalid browser sign-in URL");
                                }
                                browser = true;
                                to.send(magi_proto::HarnessEvent::Authentication {
                                    request,
                                    reply,
                                })
                                .await
                                .map_err(|_| "Authentication cancelled")?;
                            }
                            Reply::Finished { providers, .. } => {
                                if providers.len() > 1024 {
                                    return Err("Too many authentication providers");
                                }
                                result = Some(reply);
                            }
                        }
                    }
                }
                Err(e) if transient(&e) => {}
                Err(_) => return Err("Could not read authentication reply"),
            }
        }
        let status = rustix::process::waitid(
            rustix::process::WaitId::Pid(rustix::process::Pid::from_child(&child.0)),
            rustix::process::WaitidOptions::EXITED
                | rustix::process::WaitidOptions::NOHANG
                | rustix::process::WaitidOptions::NOWAIT,
        )
        .map_err(|_| "Could not inspect authentication process")?;
        if let Some(status) = status
            && eof
        {
            if status.exit_status() != Some(0) || !line.is_empty() {
                return Err(
                    "Authentication failed; update Melchior if auth --stdio is unavailable",
                );
            }
            return result.ok_or("Melchior does not provide interactive authentication; update the model-role binary");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), &'static str> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|_| "Could not inspect authentication pipe")?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| "Could not configure authentication pipe")
}

fn transient(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}

fn safe_browser_url(url: &str) -> bool {
    !url.chars().any(char::is_control)
        && (url.starts_with("https://")
            || url.starts_with("http://127.0.0.1:")
            || url.starts_with("http://localhost:"))
}

#[cfg(test)]
mod tests;
