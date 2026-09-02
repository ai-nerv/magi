//! The axon UI process.
//!
//! One multi-call binary: `axon` runs the UI, `axon fake-host` serves a recording. Tau does
//! the same with 15 components in 79 lines, and it is why out-of-process pieces still ship as
//! a single artifact.

mod app;
mod atom;
mod auth;
mod clipboard;
mod config;
mod driver;
mod ext_lua;
mod external_editor;
mod help;
mod history;
mod host;
mod keys;
mod models;
mod paths;
mod print;
mod session;
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
    /// Print the Lua client library for axon's own surface.
    ///
    /// What a sibling needs in order to talk to a running axon: framing, encoding, discovery
    /// and the verbs, as one plain-Lua file to `require`. Redirect it — `axon lua-api >
    /// config/clients/axon.lua` — because getting a file onto disk is the caller's business
    /// and a flag that picked the path would be axon inventing a convention nobody asked for.
    ///
    /// The agent surface has its own, printed by `atom lua-api`. It left with the layer.
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
    // Only for the replay host, and for a socket somebody named by hand. Every real session
    // names its own after a key nothing else holds — see [`session::socket_for`].
    let socket = cli
        .socket
        .clone()
        .unwrap_or_else(|| axon_ipc::socket_for(&cwd));

    match cli.command {
        Some(Command::Ext(Ext::Shell)) => shell::run(),

        Some(Command::Ext(Ext::Lua { file })) => ext_lua::run(&file),
        Some(Command::Auth(AuthCommand::Login { provider })) => auth::login(&provider).await,
        Some(Command::Auth(AuthCommand::Logout { provider })) => auth::logout(&provider),
        Some(Command::Auth(AuthCommand::Status)) => auth::status(),
        Some(Command::LuaApi) => {
            print!("{}", axon_lua::client::CLIENT);
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
            // Its own session like any other: journalled, and reachable by name while it runs.
            let loaded = crate::config::load().ok();
            let project =
                session::project(loaded.as_ref().and_then(|l| l.config.string("project")));
            let program = loaded
                .as_ref()
                .and_then(|l| l.config.string("atom"))
                .unwrap_or("atom")
                .to_owned();
            // Held for the run, so its socket is up while the turn is: a `-p` that another
            // session wants to ask about is one that has to be answering.
            let _layer = atom::Atom::start(&program, &project, talk(loaded.as_ref()));
            let environ = inherited(
                loaded.as_ref(),
                &_layer
                    .as_ref()
                    .map_or_else(String::new, |(layer, _)| layer.named.clone()),
            );
            let key = session::key();
            let socket = cli
                .socket
                .unwrap_or_else(|| session::socket_for(&project, &key));
            host::start(
                &socket,
                cli.sessions.as_deref(),
                cli.resume,
                &cwd,
                loaded.as_ref(),
                &environ,
                &key,
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
            let project =
                session::project(loaded.as_ref().and_then(|l| l.config.string("project")));

            // atom first, because it names this session and the name goes into the environment
            // everything else inherits. Absent, this is a session with no siblings and no
            // `agent` tool — and otherwise a working session, which is the whole point of the
            // layer being a separate program.
            let program = loaded
                .as_ref()
                .and_then(|l| l.config.string("atom"))
                .unwrap_or("atom")
                .to_owned();
            let started = atom::Atom::start(&program, &project, talk(loaded.as_ref()));
            let named = started
                .as_ref()
                .map(|(atom, _)| atom.named.clone())
                .unwrap_or_default();
            let environ = inherited(loaded.as_ref(), &named);

            // This session's own socket, named after a key nothing else shares. Named after the
            // *directory*, a second `axon` started in the same place found the first already
            // answering and joined it — one session, one journal, one transcript, and whatever
            // either of them typed appearing in both.
            let key = session::key();
            let socket = cli
                .socket
                .unwrap_or_else(|| session::socket_for(&project, &key));
            host::start(
                &socket,
                cli.sessions.as_deref(),
                cli.resume,
                &cwd,
                loaded.as_ref(),
                &environ,
                &key,
            )
            .await?;
            let ran = driver::run(&socket, cli.prompt, loaded, &project, started).await;
            // Not on a signal, and not by anybody else: the session is this process, so the
            // only thing that ends it is this process ending.
            host::done(&socket);
            ran
        }
    }
}

/// Everything this session starts inherits this, and it is how they learn which session it is.
///
/// `named` is `project/role/id` as atom gave it, or empty when atom is not installed. The three
/// variables are atom's own names for them, so `atom tool` — which is a program axon does not
/// build and does not link — finds itself without axon having to explain anything.
///
/// Empty when there is no name, rather than a plausible one: a tool that invented a name would
/// sign messages as a session that does not exist.
fn inherited(
    loaded: Option<&crate::config::Loaded>,
    named: &str,
) -> std::collections::BTreeMap<String, String> {
    let mut environ = loaded.map(crate::config::environ).unwrap_or_default();
    let mut parts = named.split('/');
    if let (Some(project), Some(role), Some(id)) = (parts.next(), parts.next(), parts.next()) {
        environ.insert("ATOM_PROJECT".to_owned(), project.to_owned());
        environ.insert("ATOM_ROLE".to_owned(), role.to_owned());
        environ.insert("ATOM_ID".to_owned(), id.to_owned());
    }
    // The `agent` tool is a separate process from the one holding the socket, and both have to
    // answer the same way about who may be reached. Set on only one of them, a refusal would
    // depend on which of the two a model happened to go through.
    if let Some(talk) = talk(loaded) {
        environ.insert(atom::TALK.to_owned(), talk.to_owned());
    }
    environ
}

/// How far this session may reach, as the config said it.
///
/// Passed through rather than parsed: the levels are the layer's vocabulary, and axon checking
/// the spelling would put the list of them in two programs.
fn talk(loaded: Option<&crate::config::Loaded>) -> Option<&str> {
    loaded.and_then(|l| l.config.string("agent_talk"))
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
