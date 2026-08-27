//! The axum UI process.
//!
//! One multi-call binary: `axum` runs the UI, `axum fake-host` serves a recording. Tau does
//! the same with 15 components in 79 lines, and it is why out-of-process pieces still ship as
//! a single artifact.

mod app;
mod config;
mod driver;
mod external_editor;
mod keys;
mod models;
mod paths;
mod terminal;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "axum", about = "A coding agent for Linux", version)]
struct Cli {
    /// Socket to connect to; defaults to `$XDG_RUNTIME_DIR/axum/host.sock`.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Which backend draws the session: `inline` keeps the terminal's own scrollback,
    /// `alt` takes the alternate screen and owns the transcript.
    #[arg(long, default_value = "alt")]
    tui: terminal::Mode,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List the providers and models axum knows about.
    Models {
        /// Include providers with no credential set.
        #[arg(long)]
        all: bool,
    },
    /// Run the daemon: own the journal, serve the socket.
    Host {
        /// Directory holding session journals.
        #[arg(long)]
        sessions: Option<PathBuf>,
    },
    /// Serve a recorded session so the UI can be developed without a model.
    FakeHost {
        /// JSONL recording to replay.
        #[arg(long)]
        replay: PathBuf,
        /// Milliseconds between events.
        #[arg(long, default_value_t = 60)]
        pace_ms: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(axum_ipc::default_socket_path);
    let mode = cli.tui;

    match cli.command {
        Some(Command::Models { all }) => {
            models::print(all);
            Ok(())
        }
        Some(Command::Host { sessions }) => {
            let dir = sessions.unwrap_or_else(axum_host::paths::sessions_dir);
            let cwd = std::env::current_dir()?.display().to_string();
            let session = axum_host::open_session(&dir, &cwd, unix_seconds())?;
            eprintln!(
                "axum host: session {} on {}",
                session.id(),
                socket.display()
            );
            let backend = crate::config::load()
                .ok()
                .and_then(|l| crate::config::backend(&l));
            if backend.is_none() {
                eprintln!("axum host: no model configured; prompts will say so");
            }
            let listener = axum_ipc::bind(&socket).await?;
            axum_host::serve(listener, session, backend).await?;
            Ok(())
        }
        Some(Command::FakeHost { replay, pace_ms }) => {
            let recording = axum_testkit::Recording::load(&replay).await?;
            eprintln!(
                "axum fake-host: {} events on {}",
                recording.len(),
                socket.display()
            );
            let listener = axum_ipc::bind(&socket).await?;
            let harness = axum_testkit::FakeHarness::new(recording, Duration::from_millis(pace_ms));
            harness.serve(listener).await?;
            Ok(())
        }
        None => driver::run(&socket, mode).await,
    }
}

/// Seconds since the epoch, for naming a session.
///
/// A session id is a sortable timestamp, which is what makes "the most recent session" a
/// directory listing rather than an index to maintain.
fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
