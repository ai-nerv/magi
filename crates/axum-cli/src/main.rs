//! The axum UI process.
//!
//! One multi-call binary: `axum` runs the UI, `axum fake-host` serves a recording. Tau does
//! the same with 15 components in 79 lines, and it is why out-of-process pieces still ship as
//! a single artifact.

mod app;
mod config;
mod daemon;
mod driver;
mod external_editor;
mod keys;
mod models;
mod paths;
mod print;
mod shell;
mod terminal;
mod tools;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "axum", about = "A coding agent for Linux", version)]
struct Cli {
    /// Socket to connect to; defaults to one named for the working directory.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Which backend draws the session: `inline` keeps the terminal's own scrollback,
    /// `alt` takes the alternate screen and owns the transcript.
    #[arg(long, default_value = "alt")]
    tui: terminal::Mode,

    /// Continue this directory's most recent session instead of starting one.
    #[arg(short, long, global = true)]
    resume: bool,

    /// Directory holding session journals.
    ///
    /// Global because the front end has to hand it to the daemon it starts, not only to
    /// a daemon someone started by hand.
    #[arg(long, global = true)]
    sessions: Option<PathBuf>,

    /// Print the answer and exit, instead of opening the UI.
    #[arg(short, long)]
    print: bool,

    /// What to ask. Submitted on start; without it the UI opens empty.
    prompt: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a tool peer. Not for people: axum spawns these itself.
    ///
    /// The multi-call shape Tau uses — out-of-process tools with single-artifact deployment,
    /// so `command = "axum"` in a declaration needs nothing else installed.
    #[command(subcommand)]
    Ext(Ext),
    /// List the tools the model can call, and how each is reached.
    Tools,
    /// List the providers and models axum knows about.
    Models {
        /// Include providers with no credential set.
        #[arg(long)]
        all: bool,
    },
    /// Run the daemon: own the journal, serve the socket.
    Host,
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
    let cwd = std::env::current_dir()?;
    let socket = cli.socket.unwrap_or_else(|| axum_ipc::socket_for(&cwd));
    let mode = cli.tui;

    match cli.command {
        Some(Command::Ext(Ext::Shell)) => shell::run(),
        Some(Command::Tools) => {
            tools::print()?;
            Ok(())
        }
        Some(Command::Models { all }) => {
            models::print(all);
            Ok(())
        }
        Some(Command::Host) => host(&socket, cli.sessions, &cwd, cli.resume).await,
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
        // A daemon is started even for a one-shot: it owns the session, so a `-p` answer is
        // journalled and resumable rather than thrown away with the process that printed it.
        None if cli.print => {
            let Some(prompt) = cli.prompt else {
                anyhow::bail!("`-p` needs a prompt: axum -p \"…\"");
            };
            daemon::ensure(&socket, cli.sessions.as_deref(), cli.resume).await?;
            let outcome = print::run(&socket, prompt).await?;
            if !outcome.text.is_empty() {
                println!("{}", outcome.text);
            }
            if let Some(error) = &outcome.error {
                eprintln!("axum: {error}");
            }
            if outcome.failed() {
                std::process::exit(1);
            }
            Ok(())
        }
        None => {
            daemon::ensure(&socket, cli.sessions.as_deref(), cli.resume).await?;
            driver::run(&socket, mode, cli.prompt).await
        }
    }
}

/// Run the daemon.
///
/// Resuming picks the newest journal recorded for this directory rather than the newest
/// anywhere: sessions are stored flat, so "the last one" on its own means the last one in
/// whatever project happened to be open most recently.
async fn host(
    socket: &std::path::Path,
    sessions: Option<PathBuf>,
    cwd: &std::path::Path,
    resume: bool,
) -> Result<()> {
    let dir = sessions.unwrap_or_else(axum_host::paths::sessions_dir);
    let cwd = cwd.display().to_string();
    let session = match resume
        .then(|| axum_host::paths::latest_for(&dir, &cwd))
        .flatten()
    {
        Some(path) => axum_host::session::Session::open(
            &path,
            axum_proto::SessionId::new(axum_host::paths::session_id(unix_seconds())),
            &cwd,
            unix_seconds(),
        )?,
        None => axum_host::open_session(&dir, &cwd, unix_seconds())?,
    };
    eprintln!(
        "axum host: session {} on {}",
        session.id(),
        socket.display()
    );
    // Fatal rather than defaulted: a config that will not run has expressed an intention that
    // has not been carried out, and a daemon that quietly ignores it answers every prompt with
    // the wrong model for as long as nobody notices.
    let loaded = crate::config::load()?;
    let backend = crate::config::backend(&loaded);
    if backend.is_none() {
        eprintln!("axum host: no model configured; prompts will say so");
    }
    let listener = axum_ipc::bind(socket).await?;
    axum_host::serve(listener, session, backend).await?;
    Ok(())
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

/// The peers axum ships.
#[derive(Subcommand)]
enum Ext {
    /// A persistent shell, spoken to over the tool protocol.
    Shell,
}
