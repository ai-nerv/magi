//! The axon UI process.
//!
//! One multi-call binary: `axon` runs the UI, `axon fake-host` serves a recording. Tau does
//! the same with 15 components in 79 lines, and it is why out-of-process pieces still ship as
//! a single artifact.

mod app;
mod auth;
mod clipboard;
mod config;
mod driver;
mod ext_lua;
mod external_editor;
mod help;
mod history;
mod host;
mod instance;
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
    /// Print the Lua client library for the agent surface.
    ///
    /// What a sibling needs in order to talk to a running axon: framing, encoding, discovery
    /// and the verbs, as one plain-Lua file to `require`. Redirect it — `axon lua-api >
    /// config/clients/agent.lua` — because getting a file onto disk is the caller's business
    /// and a flag that picked the path would be axon inventing a convention nobody asked for.
    ///
    /// The same source is served over the socket as the `client` verb, for a sandboxed VM that
    /// cannot shell out to run this. It goes with the agent layer when that leaves.
    LuaApi,
    /// List the tools the model can call, and how each is reached.
    Tools,
    /// List the providers and models axon knows about.
    Models {
        /// Include providers with no credential set.
        #[arg(long)]
        all: bool,
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
    let cwd = std::env::current_dir()?;
    // Only for a daemon somebody ran by hand, and for the replay host. Every real session names
    // its own socket after itself — see `mine` below and [`instance::host_at`].
    let socket = cli
        .socket
        .clone()
        .unwrap_or_else(|| axon_ipc::socket_for(&cwd));

    match cli.command {
        Some(Command::Ext(Ext::Shell)) => shell::run(),
        Some(Command::Ext(Ext::Agent)) => instance::peer::run(),
        Some(Command::Ext(Ext::Lua { file })) => ext_lua::run(&file),
        Some(Command::Auth(AuthCommand::Login { provider })) => auth::login(&provider).await,
        Some(Command::Auth(AuthCommand::Logout { provider })) => auth::logout(&provider),
        Some(Command::Auth(AuthCommand::Status)) => auth::status(),
        Some(Command::LuaApi) => {
            print!("{}", axon_agent::CLIENT);
            Ok(())
        }
        Some(Command::Tools) => {
            tools::print()?;
            Ok(())
        }
        Some(Command::Models { all }) => {
            models::print(all);
            Ok(())
        }
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
        // Journalled like any other session, so a `-p` answer is resumable rather than thrown
        // away with the process that printed it.
        None if cli.print => {
            let Some(prompt) = cli.prompt else {
                anyhow::bail!("`-p` needs a prompt: axon -p \"…\"");
            };
            // Its own instance too. `axon -p` is a session like any other: it has a name, it is
            // journalled, and while it runs another axon can reach it.
            let loaded = crate::config::load().ok();
            let (identity, environ) = mine(loaded.as_ref());
            let socket = cli.socket.unwrap_or_else(|| instance::host_at(&identity));
            host::start(
                &socket,
                cli.sessions.as_deref(),
                cli.resume,
                &cwd,
                loaded.as_ref(),
                &environ,
                &identity,
            )
            .await?;
            let outcome = print::run(&socket, prompt).await;
            host::done(&socket);
            let outcome = outcome?;
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
            // Loaded once, here. Every later reader is handed this one: a second `load` in the
            // same process runs every configuration file again and repeats every refusal it
            // printed the first time.
            let loaded = crate::config::load().ok();
            let (identity, environ) = mine(loaded.as_ref());
            // This instance's own socket, named after itself. Named after the *directory*, a
            // second `axon` started in the same place found the first one already answering
            // and joined it — one session, one journal, one transcript, and whatever either of
            // them typed appearing in both.
            let socket = cli.socket.unwrap_or_else(|| instance::host_at(&identity));
            host::start(
                &socket,
                cli.sessions.as_deref(),
                cli.resume,
                &cwd,
                loaded.as_ref(),
                &environ,
                &identity,
            )
            .await?;
            let ran = driver::run(&socket, cli.prompt, loaded, identity).await;
            // Not on a signal, and not by anybody else: the session is this process, so the
            // only thing that ends it is this process ending.
            host::done(&socket);
            ran
        }
    }
}

/// Who this instance is, and the environment everything it starts inherits.
///
/// Named here rather than in the UI, because the daemon is started before the first frame and
/// everything it spawns inherits this. A tool peer is the only thing that can reach other
/// instances — it needs the project directory, and the daemon does not have one — and the one
/// fact it cannot work out for itself is which session it belongs to.
fn mine(
    loaded: Option<&crate::config::Loaded>,
) -> (
    instance::Identity,
    std::collections::BTreeMap<String, String>,
) {
    let mut environ = loaded.map(crate::config::environ).unwrap_or_default();
    let identity = instance::Identity::here(loaded.and_then(|l| l.config.string("project")));
    environ.insert(instance::PROJECT.to_owned(), identity.project.clone());
    environ.insert(instance::ROLE.to_owned(), identity.role.clone());
    environ.insert(instance::ID.to_owned(), identity.id.clone());
    (identity, environ)
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
    /// The tool that reaches other axon instances.
    ///
    /// A process rather than a function in the daemon's VM, because it needs to know which
    /// instance this session is -- and only a process spawned under it inherits that.
    Agent,
    /// Tools written in Lua, served from their own process.
    ///
    /// The second implementation of the protocol, and the one that proves it is a protocol:
    /// it is a different language, a different lifecycle, and it cannot answer a `Cancel`.
    Lua {
        /// The file to load. Nothing is discovered; the config names it.
        file: PathBuf,
    },
}
