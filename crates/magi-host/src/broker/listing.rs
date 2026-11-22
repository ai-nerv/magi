use magi_proto::ask::Card;
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BYTES: usize = 16 * 1024 * 1024;
const MODELS: usize = 4096;

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

pub(super) async fn read(
    program: &str,
    environ: &BTreeMap<String, String>,
    refresh: bool,
) -> Result<(Vec<Card>, Vec<String>), String> {
    let mut command = Command::new(program);
    command
        .envs(environ)
        .args(["models", "--json"])
        .env(crate::driving::GIVEN, crate::driving::given_dir());
    if refresh {
        command.arg("--refresh");
    }
    let bytes = output(command, crate::driving::ASKED, BYTES).await?;
    decode(&bytes, refresh)
}

async fn output(mut command: Command, patience: Duration, cap: usize) -> Result<Vec<u8>, String> {
    let mut child = Owned(
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| {
                magi_model::noted!(
                    "{} could not be started",
                    command.get_program().to_string_lossy()
                );
                "Could not start the model provider.".to_owned()
            })?,
    );
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or("Model provider has no output pipe.")?;
    let flags =
        rustix::fs::fcntl_getfl(&stdout).map_err(|_| "Could not inspect model output pipe.")?;
    rustix::fs::fcntl_setfl(&stdout, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| "Could not configure model output pipe.")?;
    let deadline = Instant::now() + patience;
    let mut bytes = Vec::new();
    let mut eof = false;
    loop {
        if Instant::now() >= deadline {
            return Err("Model listing timed out; keeping the previous list.".into());
        }
        if !eof {
            let mut buffer = [0; 8192];
            match stdout.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(n) => {
                    if n > cap.saturating_sub(bytes.len()) {
                        return Err("Model listing exceeds the byte limit.".into());
                    }
                    bytes.extend_from_slice(buffer.get(..n).ok_or("Invalid model output length.")?);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err("Could not read the model listing.".into()),
            }
        }
        let status = rustix::process::waitid(
            rustix::process::WaitId::Pid(rustix::process::Pid::from_child(&child.0)),
            rustix::process::WaitidOptions::EXITED
                | rustix::process::WaitidOptions::NOHANG
                | rustix::process::WaitidOptions::NOWAIT,
        )
        .map_err(|_| "Could not inspect model provider exit status.")?;
        if let Some(status) = status
            && eof
        {
            return if status.exit_status() == Some(0) {
                Ok(bytes)
            } else {
                Err("Model provider failed; keeping the previous list.".into())
            };
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn decode(bytes: &[u8], refresh: bool) -> Result<(Vec<Card>, Vec<String>), String> {
    let reply: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "Invalid model listing; keeping the previous list.")?;
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("Model provider refused the listing; keeping the previous list.".into());
    }
    if refresh && reply.get("refreshed").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("Model provider did not confirm refresh; keeping the previous list.".into());
    }
    let rows = reply
        .get("result")
        .and_then(serde_json::Value::as_array)
        .ok_or("Invalid model cards; keeping the previous list.")?;
    if rows.len() > MODELS {
        return Err("Model listing exceeds the model count limit.".into());
    }
    let cards: Vec<Card> = serde_json::from_value(serde_json::Value::Array(rows.clone()))
        .map_err(|_| "Invalid model cards; keeping the previous list.")?;
    let failed = if refresh {
        serde_json::from_value(reply.get("failed").cloned().unwrap_or_default())
            .map_err(|_| "Invalid refresh status; keeping the previous list.")?
    } else {
        Vec::new()
    };
    Ok((cards, failed))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod input_limits;
