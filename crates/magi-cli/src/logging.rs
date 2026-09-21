//! `--logs`: every step of this session and its siblings in one file. The family reads `NERV_LOG`,
//! which a child only inherits, so this process starts itself again with it set.

use std::path::{Path, PathBuf};

/// Start logging if asked and not on already: say where, then become this same command again with
/// the family's log named. Returns only when there is nothing to do, or starting again failed.
pub fn begin(asked: Option<&Path>, verbose: bool) -> anyhow::Result<()> {
    if std::env::var_os(magi_model::noted::FAMILY).is_some() || (asked.is_none() && !verbose) {
        return Ok(());
    }
    let path = match asked.filter(|path| !path.as_os_str().is_empty()) {
        Some(path) => std::path::absolute(path)?,
        None => chosen()?,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    eprintln!("magi: logging every step to {}", path.display());
    let why = std::os::unix::process::CommandExt::exec(
        std::process::Command::new(std::env::current_exe()?)
            .args(std::env::args_os().skip(1))
            .env(magi_model::noted::FAMILY, &path),
    );
    Err(why.into())
}

/// Where a log goes when none was named: the state directory, one file per project and start.
fn chosen() -> anyhow::Result<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| anyhow::anyhow!("neither XDG_STATE_HOME nor HOME is set"))?;
    let project = std::env::current_dir()?.file_name().map_or_else(
        || "magi".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    Ok(state
        .join("nerv")
        .join("logs")
        .join(format!("{project}-{started}.log")))
}
