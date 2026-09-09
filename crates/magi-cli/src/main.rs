//! The magi UI process.
//!
//! One multi-call binary: `magi` runs the UI, `magi fake-host` serves a recording. Tau does
//! the same with 15 components in 79 lines, and it is why out-of-process pieces still ship as
//! a single artifact.

mod app;
mod balthasar;
mod clipboard;
mod config;
mod doctor;
mod driver;
mod driving;
mod ext_lua;
mod external_editor;
mod help;
mod history;
mod host;
mod keying;
mod keys;
mod melchior;
mod models;
mod opening;
mod paths;
mod print;
mod session;
mod shell;
mod terminal;
mod tools;
mod ui;
mod verbs;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "magi", about = "A coding agent for Linux", version)]
struct Cli {
    /// Socket to connect to; defaults to one named for the working directory.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Continue this directory's most recent session instead of starting one.
    #[arg(short, long, global = true)]
    resume: bool,

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
    /// Run a tool peer. Not for people: magi spawns these itself.
    ///
    /// The multi-call shape Tau uses — out-of-process tools with single-artifact deployment,
    /// so `command = "magi"` in a declaration needs nothing else installed.
    #[command(subcommand)]
    Ext(Ext),
    /// Print the Lua client library for magi's own surface.
    ///
    /// What a sibling needs in order to talk to a running magi: framing, encoding, discovery
    /// and the verbs, as one plain-Lua file to `require`. Redirect it — `magi lua-api >
    /// config/clients/magi.lua` — because getting a file onto disk is the caller's business
    /// and a flag that picked the path would be magi inventing a convention nobody asked for.
    ///
    /// The agent surface has its own, printed by `melchior lua-api`. It left with the layer.
    ///
    /// `client` is the family's name for it; `lua-api` is what this program called it first.
    #[command(alias = "client")]
    LuaApi,
    /// Every verb this program answers, on each of its doors.
    ///
    /// magi coordinates rather than being coordinated, so it answers the floor of the family
    /// contract and not `needs` or `configure` — see FAMILY.md. It answers `verbs` for the same
    /// reason every sibling does: a family where one program can be asked what it speaks and
    /// another cannot has stopped being one.
    Verbs {
        /// Answer in JSON. The default, and accepted so every sibling takes the same flags.
        #[arg(long)]
        json: bool,
        /// Answer in CBOR rather than JSON.
        #[arg(long)]
        cbor: bool,
    },
    /// List the tools the model can call, and how each is reached.
    Tools,
    /// Acknowledge the installed packages, so they may run.
    ///
    /// A file in your own `plugin/` directory runs on sight -- you put it there. A package under
    /// `site/pack/` is somebody else's code that arrived by being fetched, so it runs once you
    /// have said it may, and stops running again the moment it changes. This is where you say so.
    ///
    /// Prints what it acknowledged. Run it after installing or updating anything.
    Acknowledge,
    /// Say what a session here would be made of, without starting one.
    ///
    /// Which configuration was read, which of its lines were kept, what the tool registry ends
    /// up holding and where each entry came from, and whether the siblings are actually
    /// answering. Everything a session decides at start-up, decided and printed rather than
    /// discovered by noticing that something is missing.
    Doctor,
    /// List the providers and models magi knows about.
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

/// **Not `#[tokio::main]`, and the reason is the prologue.**
///
/// melchior names this session, and that name is what the run and the agent are taken from — so
/// it has to be settled before the balthasar those are filed in is spawned. Naming it in the
/// prologue makes the order a shape rather than a rule to remember: there is nowhere later to
/// put it.
fn main() -> Result<()> {
    let cli = Cli::parse();
    // Only a session has a prologue. Skipped for the argument error a `-p` with no prompt is, so
    // the complaint arrives without a configuration having been read or a layer started for a
    // session that never opens.
    let opening = (cli.command.is_none() && !(cli.print && cli.prompt.is_none()))
        .then(opening::Opening::begin);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli, opening))
}

async fn run(cli: Cli, opening: Option<opening::Opening>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    // Only for the replay host, and for a socket somebody named by hand. Every real session
    // names its own after a key nothing else holds — see [`session::socket_for`].
    let socket = cli
        .socket
        .clone()
        .unwrap_or_else(|| magi_ipc::socket_for(&cwd));

    match cli.command {
        Some(Command::Ext(Ext::Shell)) => shell::run(),

        Some(Command::Ext(Ext::Lua { file })) => ext_lua::run(&file),
        Some(Command::LuaApi) => {
            print!("{}", magi_lua::client::CLIENT);
            Ok(())
        }
        Some(Command::Verbs { cbor, .. }) => {
            verbs::print(cbor);
            Ok(())
        }
        Some(Command::Acknowledge) => {
            config::acknowledge();
            Ok(())
        }
        Some(Command::Tools) => {
            tools::print()?;
            Ok(())
        }
        Some(Command::Doctor) => {
            doctor::print();
            Ok(())
        }
        Some(Command::Models { all }) => {
            models::print(all);
            Ok(())
        }
        Some(Command::FakeHost { replay, pace_ms }) => {
            let recording = magi_testkit::Recording::load(&replay).await?;
            eprintln!(
                "magi fake-host: {} events on {}",
                recording.len(),
                socket.display()
            );
            let listener = magi_ipc::bind(&socket).await?;
            let harness = magi_testkit::FakeHarness::new(recording, Duration::from_millis(pace_ms));
            harness.serve(listener).await?;
            Ok(())
        }
        // Journalled like any other session, so a `-p` answer is resumable rather than thrown
        // away with the process that printed it.
        None if cli.print => {
            let Some(prompt) = cli.prompt else {
                anyhow::bail!("`-p` needs a prompt: magi -p \"…\"");
            };
            // Its own session like any other: journalled, and reachable by name while it runs.
            let opening = opening.expect("a session's prologue runs before the runtime");
            let loaded = opening.loaded;
            let project = opening.project;
            // Held for the run, so its socket is up while the turn is: a `-p` that another
            // session wants to ask about is one that has to be answering.
            let _layer = opening.started;
            let environ = inherited(loaded.as_ref(), &opening.named);
            let key = session::key();
            let socket = cli
                .socket
                .unwrap_or_else(|| session::socket_for(&project, &key));
            // **Reaped even when it will not serve.** `start` now refuses a session it cannot
            // record, and it refuses *after* convening balthasar — so returning the error here
            // would leave the child this process started running with nothing to talk to. It
            // dies with its magi either way; this is the way that does not wait for a signal.
            if let Err(why) = host::start(
                &socket,
                cli.resume,
                &cwd,
                loaded.as_ref(),
                &environ,
                host::Named {
                    key: &key,
                    run: opening.run.as_deref(),
                    agent: opening.agent.as_deref(),
                },
            )
            .await
            {
                balthasar::stop();
                return Err(why);
            }
            let outcome = print::run(&socket, prompt).await;
            // Before the socket goes: the turn's own flush runs on a spawned task, which a
            // process exiting this promptly can outrun.
            magi_host::drain().await;
            balthasar::stop();
            host::done(&socket);
            let outcome = outcome?;
            if !outcome.text.is_empty() {
                println!("{}", outcome.text);
            }
            if let Some(error) = &outcome.error {
                eprintln!("magi: {error}");
            }
            if outcome.failed() {
                std::process::exit(1);
            }
            Ok(())
        }
        None => {
            // The configuration, the layer and this session's name, all settled before the
            // runtime existed — see [`opening`].
            let opening = opening.expect("a session's prologue runs before the runtime");
            let loaded = opening.loaded;
            let project = opening.project;
            let started = opening.started;
            let environ = inherited(loaded.as_ref(), &opening.named);

            // This session's own socket, named after a key nothing else shares. Named after the
            // *directory*, a second `magi` started in the same place found the first already
            // answering and joined it — one session, one journal, one transcript, and whatever
            // either of them typed appearing in both.
            let key = session::key();
            let socket = cli
                .socket
                .unwrap_or_else(|| session::socket_for(&project, &key));
            // **Reaped even when it will not serve.** `start` now refuses a session it cannot
            // record, and it refuses *after* convening balthasar — so returning the error here
            // would leave the child this process started running with nothing to talk to. It
            // dies with its magi either way; this is the way that does not wait for a signal.
            if let Err(why) = host::start(
                &socket,
                cli.resume,
                &cwd,
                loaded.as_ref(),
                &environ,
                host::Named {
                    key: &key,
                    run: opening.run.as_deref(),
                    agent: opening.agent.as_deref(),
                },
            )
            .await
            {
                balthasar::stop();
                return Err(why);
            }
            let ran = driver::run(&socket, cli.prompt, loaded, &project, started).await;
            // Not on a signal, and not by anybody else: the session is this process, so the
            // only thing that ends it is this process ending.
            magi_host::drain().await;
            balthasar::stop();
            host::done(&socket);
            ran
        }
    }
}

/// Everything this session starts inherits this, and it is how they learn which session it is.
///
/// `named` is `project/role/id` as melchior gave it, or empty when melchior is not installed. The three
/// variables are melchior's own names for them, so `melchior tool` — which is a program magi does not
/// build and does not link — finds itself without magi having to explain anything.
///
/// Empty when there is no name, rather than a plausible one: a tool that invented a name would
/// sign messages as a session that does not exist.
///
/// **`BALTHASAR_AGENT` is deliberately not here.** balthasar reads the agent out of the
/// *connecting* process's environment, and the memory tools are functions in this process's own
/// Lua VM — so the map handed to children is the one place setting it would have no effect on
/// the connections that matter. It is established in this process's own environment instead,
/// which children inherit anyway. See [`crate::balthasar::pin_agent`].
fn inherited(
    loaded: Option<&crate::config::Loaded>,
    named: &str,
) -> std::collections::BTreeMap<String, String> {
    let mut environ = loaded.map(crate::config::environ).unwrap_or_default();
    let mut parts = named.split('/');
    if let (Some(project), Some(role), Some(id)) = (parts.next(), parts.next(), parts.next()) {
        environ.insert("MAGI_MELCHIOR_PROJECT".to_owned(), project.to_owned());
        environ.insert("MAGI_MELCHIOR_ROLE".to_owned(), role.to_owned());
        environ.insert("MAGI_MELCHIOR_ID".to_owned(), id.to_owned());
    }
    // The `agent` tool is a separate process from the one holding the socket, and both have to
    // answer the same way about who may be reached. Set on only one of them, a refusal would
    // depend on which of the two a model happened to go through.
    if let Some(talk) = talk(loaded) {
        environ.insert(melchior::TALK.to_owned(), talk.to_owned());
    }
    environ
}

/// How far this session may reach, as the config said it.
///
/// Passed through rather than parsed: the levels are the layer's vocabulary, and magi checking
/// the spelling would put the list of them in two programs.
fn talk(loaded: Option<&crate::config::Loaded>) -> Option<&str> {
    loaded.and_then(|l| l.config.string("agent_talk"))
}

/// The peers magi ships.
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

/// What a session hands its children, and what it must keep for itself.
#[cfg(test)]
mod inheriting {
    use super::*;

    #[test]
    fn the_three_melchior_names_go_to_everything_this_session_starts() {
        let environ = inherited(None, "magi/main/alpha-rho");
        assert_eq!(
            environ.get("MAGI_MELCHIOR_PROJECT").map(String::as_str),
            Some("magi")
        );
        assert_eq!(
            environ.get("MAGI_MELCHIOR_ROLE").map(String::as_str),
            Some("main")
        );
        assert_eq!(
            environ.get("MAGI_MELCHIOR_ID").map(String::as_str),
            Some("alpha-rho")
        );
    }

    /// **This session's agent is not one to hand down.**
    ///
    /// Everything else in this map is the same for a session and everything it starts — the
    /// project, the run, how far either may reach. The agent is the one thing that is different
    /// for each of them, and a child that inherited its parent's would file its scratch in the
    /// parent's directory: the separation would be on disk and absent from the answers, which is
    /// the whole of what the agent dimension exists to give. A child is told its own name when
    /// it is spawned, by whoever named it.
    #[test]
    fn the_agent_is_not_something_a_session_hands_its_children() {
        let environ = inherited(None, "magi/main/alpha-rho");
        assert!(
            !environ.contains_key(crate::balthasar::AGENT),
            "a child inherited its parent's agent and would file scratch in its directory"
        );
    }

    #[test]
    fn a_session_with_no_name_hands_down_none_of_them() {
        // A tool that invented a name would sign messages as a session that does not exist.
        let environ = inherited(None, "");
        assert!(!environ.contains_key("MAGI_MELCHIOR_ID"));
        assert!(!environ.contains_key(crate::balthasar::AGENT));
    }
}
