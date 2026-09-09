//! The magi UI process: one multi-call binary, so out-of-process pieces ship as a single artifact.

mod app;
mod balthasar;
mod child;
mod clipboard;
mod config;
mod doctor;
mod driver;
mod driving;
mod ext_lua;
mod external_editor;
mod forking;
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

    /// Serve this session with no terminal, and stay reachable until something ends it.
    #[arg(long)]
    headless: bool,

    /// Which process this session must not outlive. `magi fork` sets it; implies `--headless`.
    #[arg(long, hide = true, value_name = "PID")]
    tied: Option<u32>,

    /// What this session is for, in one word. `main` when nothing says. Written in at birth.
    #[arg(long, value_name = "NAME")]
    role: Option<String>,

    /// What that role means, in a sentence a coordinator can route by.
    #[arg(long, value_name = "TEXT")]
    role_description: Option<String>,

    /// What to ask. Submitted on start; without it the UI opens empty.
    prompt: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a tool peer. Not for people: magi spawns these itself.
    #[command(subcommand)]
    Ext(Ext),
    /// Print the Lua client library for magi's own surface, as one plain-Lua file to `require`.
    #[command(alias = "client")]
    LuaApi,
    /// Every verb this program answers, on each of its doors.
    Verbs {
        /// Answer in JSON. The default, and accepted so every sibling takes the same flags.
        #[arg(long)]
        json: bool,
        /// Answer in CBOR rather than JSON.
        #[arg(long)]
        cbor: bool,
    },
    /// Start a child session of this one, and print what it is called.
    ///
    /// melchior names it and mints the secret that makes it stoppable; magi starts the process.
    Fork {
        /// What the child is for, in one word. `main` when nothing says. Given at birth.
        #[arg(long)]
        role: Option<String>,
        /// What that role means, in a sentence a coordinator can route by.
        #[arg(long)]
        role_description: Option<String>,
        /// What the child should get on with. Without it, it comes up idle and waits to be told.
        prompt: Option<String>,
    },
    /// List the tools the model can call, and how each is reached.
    Tools,
    /// Acknowledge the installed packages, so they may run.
    ///
    /// A package under `site/pack/` runs once you have said it may, and stops when it changes.
    Acknowledge,
    /// Say what a session here would be made of, without starting one.
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

/// Not `#[tokio::main]`: melchior names this session in the prologue, and the run and agent are
/// taken from that name, so it is settled before balthasar is spawned.
fn main() -> Result<()> {
    let cli = Cli::parse();
    // Only a session has a prologue, so an argument error arrives without a layer being started.
    let opening = (cli.command.is_none() && !(cli.print && cli.prompt.is_none())).then(|| {
        opening::Opening::begin(
            cli.socket.clone(),
            melchior::Role {
                name: cli.role.as_deref(),
                description: cli.role_description.as_deref(),
            },
        )
    });
    // `fork` is not a session: a prologue here would name a second one and throw it away.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli, opening))
}

async fn run(cli: Cli, opening: Option<opening::Opening>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    // Only for the replay host and a socket named by hand; every real session names its own.
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
        Some(Command::Fork {
            role,
            role_description,
            prompt,
        }) => forking::fork(
            role.as_deref(),
            role_description.as_deref(),
            prompt.as_deref(),
        ),
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
        // Journalled like any other session, so a `-p` answer is resumable.
        None if cli.print => {
            let Some(prompt) = cli.prompt else {
                anyhow::bail!("`-p` needs a prompt: magi -p \"…\"");
            };
            let opening = opening.expect("a session's prologue runs before the runtime");
            let loaded = opening.loaded;
            // Held for the run, so its socket is up while the turn is.
            let _layer = opening.started;
            let environ = inherited(loaded.as_ref(), &opening.named);
            // Named in the prologue, because melchior publishes it there — see [`opening`].
            let key = opening.key;
            let socket = opening.socket;
            // Reaped even when it will not serve: `start` refuses after convening balthasar.
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
            // Before the socket goes: the turn's own flush runs on a task this exit can outrun.
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
            // The configuration, the layer and this session's name, settled before the runtime.
            let opening = opening.expect("a session's prologue runs before the runtime");
            let loaded = opening.loaded;
            let project = opening.project;
            let started = opening.started;
            let environ = inherited(loaded.as_ref(), &opening.named);

            // Named after a key nothing else shares; named after the directory, a second `magi` in
            // the same place joined the first. Settled in the prologue — see [`opening`].
            let key = opening.key;
            let socket = opening.socket;
            // Reaped even when it will not serve: `start` refuses after convening balthasar.
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
            // The same session either way: bound, announced and recorded before anything looks.
            let ran = if headless(&cli) {
                child::run(&socket, cli.prompt, started, cli.tied).await
            } else {
                driver::run(&socket, cli.prompt, loaded, &project, started).await
            };
            // Not on a signal: the session is this process, so only this process ending ends it.
            magi_host::drain().await;
            balthasar::stop();
            host::done(&socket);
            ran
        }
    }
}

/// Everything this session starts inherits this, under melchior's own names for the variables.
/// Empty when there is no name, because a tool that invented one would sign as a session that does
/// not exist. `BALTHASAR_AGENT` is deliberately absent: balthasar reads the agent out of the
/// *connecting* process's environment, and the memory tools run in this process's own Lua VM.
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
    // The `agent` tool is a separate process from the one holding the socket, and both must agree.
    if let Some(talk) = talk(loaded) {
        environ.insert(melchior::TALK.to_owned(), talk.to_owned());
    }
    // Which process this session *is*: `magi fork` runs a shell or two below it and cannot tell.
    environ.insert(
        crate::forking::SESSION_PID.to_owned(),
        std::process::id().to_string(),
    );
    environ
}

/// Whether this session comes up without a terminal. `--tied` implies `--headless`.
fn headless(cli: &Cli) -> bool {
    cli.headless || cli.tied.is_some()
}

/// How far this session may reach: passed through unparsed, since the levels are the layer's.
fn talk(loaded: Option<&crate::config::Loaded>) -> Option<&str> {
    loaded.and_then(|l| l.config.string("agent_talk"))
}

/// The peers magi ships.
#[derive(Subcommand)]
enum Ext {
    /// A persistent shell, spoken to over the tool protocol.
    Shell,
    /// Tools written in Lua, served from their own process; the peer that cannot answer a `Cancel`.
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

    /// A child that inherited its parent's agent would file its scratch in the parent's directory.
    #[test]
    fn the_agent_is_not_something_a_session_hands_its_children() {
        let environ = inherited(None, "magi/main/alpha-rho");
        assert!(
            !environ.contains_key(crate::balthasar::AGENT),
            "a child inherited its parent's agent and would file scratch in its directory"
        );
    }

    /// What a child needs is the pid of the *session*, not of whichever shell is between them.
    #[test]
    fn the_process_this_session_is_goes_to_everything_it_starts() {
        let environ = inherited(None, "magi/main/alpha-rho");
        assert_eq!(
            environ.get(crate::forking::SESSION_PID).map(String::as_str),
            Some(std::process::id().to_string().as_str()),
            "a fork could not tell a child what to outlive"
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
