//! `axon ext agent` — the peer that lets the model reach other instances.
//!
//! A tool in its own process, speaking the same five-message protocol as `shell`. It exists in
//! a process rather than as a function in the daemon's VM for a reason that took a while to
//! find: **the tool has to run where the instance directory means something.**
//!
//! The daemon holds the tool registry, and the daemon does not know which instance this session
//! is — the identity is made when a UI starts, and a daemon outlives UIs. A peer is spawned by
//! the daemon and inherits its environment, which carries [`PROJECT`](axon_agent::directory::PROJECT) and [`ID`](axon_agent::directory::ID)
//! down from the UI that started it. So the peer knows who it is, can read the project
//! directory, and can dial. None of that is true inside the daemon.
//!
//! # What it knows, and where each part comes from
//!
//! | | |
//! |---|---|
//! | who this session is | the environment, put there by the UI that bound the socket |
//! | who started it | [`PARENT`](axon_agent::directory::PARENT), inherited across the spawn |
//! | what it started | the project directory: every socket whose note names this session |
//! | what has arrived | asked of this session's own socket |
//!
//! Only the first is taken on trust, and only because there is nothing better: a process cannot
//! be asked which UI started the daemon that started it. Everything else is read from somewhere
//! that cannot be talked into lying.

use super::Agent;
use axon_agent::verbs::Standing;
use axon_ipc::blocking::{FrameReader, FrameWriter};
use axon_proto::{ToolReport, ToolRequest};
use axon_tools::Tool;

/// Run the peer until its input closes.
///
/// One call at a time, unlike `shell`: every call here is a short round trip on a local socket
/// with its own timeout, so there is nothing to interrupt and no state to keep between calls.
/// A `Cancel` is read and dropped rather than ignored — the frame has to leave the pipe.
pub fn run() -> anyhow::Result<()> {
    let mut writer = FrameWriter::new(std::io::stdout());
    let agent = Agent {
        standing: Standing::default(),
    };
    writer.write_blocking(&ToolReport::Declare {
        name: agent.name().to_owned(),
        description: agent.description().to_owned(),
        parameters: agent.parameters(),
    })?;

    let mut reader = FrameReader::new(std::io::stdin());
    loop {
        let request: ToolRequest = match reader.read_blocking() {
            Ok(request) => request,
            // The host went away, which is the ordinary end of a peer.
            Err(_) => return Ok(()),
        };
        let ToolRequest::Call {
            id,
            name: _,
            arguments,
        } = request
        else {
            continue;
        };
        // Gathered per call, not once at startup. A session forks children while it runs and
        // its inbox fills while it thinks; a peer that read both when it started would answer
        // `list` with the world as it was when the daemon happened to spawn it.
        let out = match standing() {
            Some(standing) => Agent { standing }.run(&arguments, &Nothing, &Nothing),
            // Said once, plainly, rather than let through to produce `This session is ``` and a
            // list of nobody. A peer that does not know which session it is cannot sign a
            // message, and inventing a name would put one in somebody's inbox from a sender
            // that does not exist.
            None => axon_tools::Output::error(format!(
                "this process was not started by an axon session, so it does not know which \
                 instance it would be speaking as. `{}` and `{}` are set by the session that \
                 starts the daemon.",
                super::PROJECT,
                super::ID
            )),
        };
        writer.write_blocking(&ToolReport::Result {
            id,
            output: out.content,
            is_error: out.is_error,
        })?;
    }
}

/// What this session is, as far as a separate process can tell.
///
/// `None` when the environment does not say which session this belongs to, which means nobody
/// started it as part of one.
fn standing() -> Option<Standing> {
    let me = super::mine()?;
    Some(Standing {
        inbox: super::inbox_of(&me),
        forked: super::children(&me),
        parent: super::parent(),
        // Empty, and so `stop` is refused with "this session did not start it". The secrets are
        // minted by whatever spawns a child, and nothing spawns one yet; when something does,
        // this is where they arrive.
        minted: std::collections::BTreeMap::new(),
        me: me.full(),
    })
}

/// A tool peer has no host to reach back into.
///
/// `Ops` is a session`s filesystem and shell, and this tool touches neither -- everything it
/// does is a round trip on a socket. Every method refuses rather than pretending, so a verb
/// that grew a file read later fails loudly here instead of quietly reading the wrong tree.
struct Nothing;

impl axon_tools::Cancel for Nothing {
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl axon_tools::Ops for Nothing {
    fn cwd(&self) -> std::path::PathBuf {
        std::env::current_dir().unwrap_or_default()
    }

    fn read(&self, _path: &std::path::Path) -> Result<String, String> {
        Err("the agent tool does not read files".to_owned())
    }

    fn write(&self, _path: &std::path::Path, _contents: &str) -> Result<(), String> {
        Err("the agent tool does not write files".to_owned())
    }

    fn shell(&self, _command: &str) -> Result<axon_tools::Shell, String> {
        Err("the agent tool does not run commands".to_owned())
    }
}
