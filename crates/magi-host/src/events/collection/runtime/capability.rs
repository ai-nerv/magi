use super::Owner;
use crate::events::Revision;
use serde::Deserialize;
use std::{
    io::{self, Read},
    os::unix::{net::UnixStream, process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const LIMIT: usize = 512;
const PATIENCE: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    ok: bool,
    family: Revision,
    n: u8,
    result: [Capabilities; 1],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    schema: Revision,
    kind: Kind,
    program: Owner,
    bootstrap: u8,
}

#[derive(Deserialize)]
enum Kind {
    #[serde(rename = "native_event_capabilities")]
    Native,
}

fn accepts(bytes: &[u8], owner: Owner) -> bool {
    if bytes.len() > LIMIT {
        return false;
    }
    let Ok(reply) = serde_json::from_slice::<Reply>(bytes) else {
        return false;
    };
    let [capabilities] = reply.result;
    let _ = (
        reply.family,
        capabilities.schema,
        capabilities.kind,
        capabilities.bootstrap,
    );
    reply.ok && reply.n == 1 && capabilities.program == owner && capabilities.bootstrap == 2
}

pub(super) fn supported(command: &Command, owner: Owner) -> bool {
    probe(command).is_ok_and(|bytes| accepts(&bytes, owner))
}

struct Probe(std::process::Child);

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(pid) = i32::try_from(self.0.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
        }
        let _ = self.0.wait();
    }
}

fn probe(selected: &Command) -> io::Result<Vec<u8>> {
    let (mut reader, writer) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    let mut command = Command::new(selected.get_program());
    command
        .arg("event-capabilities")
        .process_group(0)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::os::fd::OwnedFd::from(writer)))
        .stderr(Stdio::null());
    if let Some(cwd) = selected.get_current_dir() {
        command.current_dir(cwd);
    }
    let path = selected.get_envs().find(|(key, _)| *key == "PATH");
    match path {
        Some((_, Some(value))) => {
            command.env("PATH", value);
        }
        Some((_, None)) => {}
        None => {
            if let Some(value) = std::env::var_os("PATH") {
                command.env("PATH", value);
            }
        }
    }
    let child = Probe(command.spawn()?);
    drop(command);
    let pid = i32::try_from(child.0.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or(io::ErrorKind::InvalidData)?;
    let deadline = Instant::now() + PATIENCE;
    let mut bytes = [0; LIMIT + 1];
    let mut len = 0;
    let mut eof = false;
    loop {
        match reader.read(bytes.get_mut(len..).ok_or(io::ErrorKind::InvalidData)?) {
            Ok(0) => eof = true,
            Ok(n) => len += n,
            Err(why) if why.kind() == io::ErrorKind::WouldBlock => {}
            Err(why) => return Err(why),
        }
        if len > LIMIT {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if eof
            && let Some(status) = rustix::process::waitid(
                rustix::process::WaitId::Pid(pid),
                rustix::process::WaitidOptions::EXITED
                    | rustix::process::WaitidOptions::NOHANG
                    | rustix::process::WaitidOptions::NOWAIT,
            )?
        {
            return if status.exit_status() == Some(0) {
                Ok(bytes.get(..len).unwrap_or_default().to_vec())
            } else {
                Err(io::ErrorKind::Unsupported.into())
            };
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests;
