use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

fn birth(process: &Path) -> Result<Option<u64>> {
    let stat = match std::fs::read_to_string(process.join("stat")) {
        Ok(stat) => stat,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    Ok(Some(
        stat.rsplit_once(')')
            .context("process command boundary")?
            .1
            .split_whitespace()
            .nth(19)
            .context("process start time")?
            .parse()?,
    ))
}

fn blocked_readers(barrier: &Path) -> Result<Vec<(PathBuf, u64)>> {
    use std::os::unix::ffi::OsStrExt;
    let argument = barrier.as_os_str().as_bytes();
    let mut readers = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let process = entry?.path();
        let Ok(command) = std::fs::read(process.join("cmdline")) else {
            continue;
        };
        if command
            .split(|byte| *byte == 0)
            .any(|part| part == argument)
            && let Some(start) = birth(&process)?
        {
            readers.push((process, start));
        }
    }
    Ok(readers)
}

pub async fn started(root: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let barrier = root.join("a/tool-barrier");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let readers = blocked_readers(&barrier)?;
            if root.join("a/tool-started").exists() && !readers.is_empty() {
                return Ok(readers);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("approved shell did not enter its blocking read")?
}

pub async fn reaped(readers: &[(PathBuf, u64)]) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut remaining = Vec::new();
        for (process, start) in readers {
            if birth(process)? == Some(*start) {
                remaining.push(process);
            }
        }
        if remaining.is_empty() {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "owned tool readers survived: {remaining:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
