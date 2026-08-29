//! The axon UI process.
//!
//! One multi-call binary: `axon` runs the UI, `axon fake-host` serves a recording. Tau does
//! the same with 15 components in 79 lines, and it is why out-of-process pieces still ship as
//! a single artifact.

mod app;
mod auth;
mod config;
mod daemon;
mod driver;
mod ext_lua;
mod external_editor;
mod help;
mod history;
mod keys;
mod models;
mod paths;
mod print;
mod shell;
mod stop;
mod terminal;
mod tools;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "axon", about = "A coding agent for Linux", version)]
struct Cli {
    /// Socket to connect to; defaults to one named for the working directory.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

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
    /// Run a tool peer. Not for people: axon spawns these itself.
    ///
    /// The multi-call shape Tau uses — out-of-process tools with single-artifact deployment,
    /// so `command = "axon"` in a declaration needs nothing else installed.
    #[command(subcommand)]
    Ext(Ext),
    /// Sign in to a provider that uses a subscription rather than a key.
    #[command(subcommand)]
    Auth(AuthCommand),
    /// Stop the daemon for this directory.
    ///
    /// Quitting the UI is a detach on purpose — the turn keeps running — so nothing otherwise
    /// ever ends one, and a week of work leaves a process per project.
    Stop {
        /// Stop every daemon, not just this directory's.
        #[arg(long)]
        all: bool,
    },
    /// List the tools the model can call, and how each is reached.
    Tools,
    /// List the providers and models axon knows about.
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
    let socket = cli.socket.unwrap_or_else(|| axon_ipc::socket_for(&cwd));

    match cli.command {
        Some(Command::Ext(Ext::Shell)) => shell::run(),
        Some(Command::Ext(Ext::Lua { file })) => ext_lua::run(&file),
        Some(Command::Auth(AuthCommand::Login { provider })) => auth::login(&provider).await,
        Some(Command::Auth(AuthCommand::Logout { provider })) => auth::logout(&provider),
        Some(Command::Auth(AuthCommand::Status)) => auth::status(),
        Some(Command::Stop { all }) => stop::run(&socket, all),
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
            let recording = axon_testkit::Recording::load(&replay).await?;
            eprintln!(
                "axon fake-host: {} events on {}",
                recording.len(),
                socket.display()
            );
            let listener = axon_ipc::bind(&socket).await?;
            let harness = axon_testkit::FakeHarness::new(recording, Duration::from_millis(pace_ms));
            harness.serve(listener).await?;
            Ok(())
        }
        // A daemon is started even for a one-shot: it owns the session, so a `-p` answer is
        // journalled and resumable rather than thrown away with the process that printed it.
        None if cli.print => {
            let Some(prompt) = cli.prompt else {
                anyhow::bail!("`-p` needs a prompt: axon -p \"…\"");
            };
            daemon::ensure(&socket, cli.sessions.as_deref(), cli.resume).await?;
            let outcome = print::run(&socket, prompt).await?;
            if !outcome.text.is_empty() {
                println!("{}", outcome.text);
            }
            if let Some(error) = &outcome.error {
                eprintln!("axon: {error}");
            }
            if outcome.failed() {
                std::process::exit(1);
            }
            Ok(())
        }
        None => {
            // The return value is not kept: the UI stops this directory's daemon when it
            // exits whether it started it or adopted one that was already there.
            daemon::ensure(&socket, cli.sessions.as_deref(), cli.resume).await?;
            driver::run(&socket, cli.prompt, cli.sessions).await
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
    let dir = sessions.unwrap_or_else(axon_host::paths::sessions_dir);
    let cwd = cwd.display().to_string();
    let session = match resume
        .then(|| axon_host::paths::latest_for(&dir, &cwd))
        .flatten()
    {
        Some(path) => axon_host::session::Session::open(
            &path,
            axon_proto::SessionId::new(axon_host::paths::session_id(unix_seconds())),
            &cwd,
            unix_seconds(),
        )?,
        None => axon_host::open_session(&dir, &cwd, unix_seconds())?,
    };
    eprintln!(
        "axon host: session {} on {}",
        session.id(),
        socket.display()
    );
    // Fatal rather than defaulted: a config that will not run has expressed an intention that
    // has not been carried out, and a daemon that quietly ignores it answers every prompt with
    // the wrong model for as long as nobody notices.
    let loaded = crate::config::load()?;
    let backend = crate::config::backend(&loaded);
    let catalog = crate::config::catalog(&loaded);
    if backend.is_none() {
        eprintln!("axon host: no model configured; prompts will say so");
    }
    let listener = axon_ipc::bind(socket).await?;

    // Raced against the signals that mean "stop", so the daemon takes its socket and pid file
    // with it. Left behind, a socket nobody is listening on is indistinguishable from a daemon
    // that is merely busy, and the next run waits out its whole startup timeout on it.
    tokio::select! {
        result = axon_host::serve_catalog(listener, session, backend, catalog) => result?,
        () = shutdown() => eprintln!("axon host: stopping"),
    }
    let _ = tokio::fs::remove_file(socket).await;
    let _ = tokio::fs::remove_file(daemon::pid_path(socket)).await;
    Ok(())
}

/// Resolve when the daemon is asked to stop.
///
/// Both signals, because `axon stop` sends one and a person with the daemon in the foreground
/// sends the other, and neither should leave files behind.
async fn shutdown() {
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(term) => term,
        // Nothing can be installed, so nothing can be waited on. Never resolving is right: the
        // other arm of the race is the daemon doing its job.
        Err(_) => return std::future::pending().await,
    };
    tokio::select! {
        _ = term.recv() => {}
        result = tokio::signal::ctrl_c() => {
            if result.is_err() {
                std::future::pending::<()>().await;
            }
        }
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

/// What `axon auth` can do.
#[derive(Subcommand)]
enum AuthCommand {
    /// Sign in, through your browser.
    Login {
        /// The provider, as `axon models` names it.
        provider: String,
    },
    /// Forget a provider's credentials.
    Logout {
        /// The provider to forget.
        provider: String,
    },
    /// Show which providers are signed in.
    Status,
}

/// The peers axon ships.
#[derive(Subcommand)]
enum Ext {
    /// A persistent shell, spoken to over the tool protocol.
    Shell,
    /// Tools written in Lua, served from their own process.
    ///
    /// The second implementation of the protocol, and the one that proves it is a protocol:
    /// it is a different language, a different lifecycle, and it cannot answer a `Cancel`.
    Lua {
        /// The file to load. Nothing is discovered; the config names it.
        file: PathBuf,
    },
}
