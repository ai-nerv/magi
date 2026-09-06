//! Tools from an MCP server.
//!
//! **The one interoperability gap that matters.** Everything else this family does is its own —
//! its own wire, its own directory, its own walls — and that is defensible because those are
//! about sessions on one machine. Tools are different: the ecosystem settled, MCP won, and a
//! harness that cannot run an MCP server is one a person has to leave to use a filesystem
//! browser somebody else already wrote.
//!
//! **Nothing else changes.** A transport is a property of a declaration, not a second registry —
//! so an MCP tool registers beside a builtin, a Lua tool and a casper tool, is checked against
//! the same schema, asks the same person for the same permission, is capped by the same
//! [`crate::bound`] and masked by the same [`crate::masking`]. The turn loop does not know MCP
//! exists.
//!
//! # The protocol, as much of it as a client needs
//!
//! JSON-RPC 2.0, one object per line, over the server's stdin and stdout. Three calls:
//!
//! ```text
//! -> {"jsonrpc":"2.0","id":1,"method":"initialize","params":{…}}
//! <- {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"…","serverInfo":{…}}}
//! -> {"jsonrpc":"2.0","method":"notifications/initialized"}
//! -> {"jsonrpc":"2.0","id":2,"method":"tools/list"}
//! <- {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":…,"inputSchema":…}]}}
//! -> {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":…,"arguments":…}}
//! <- {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":…}]}}
//! ```
//!
//! Newline-delimited, not length-prefixed: that is MCP's framing and it is the one part of this
//! that cannot be chosen. A server may write anything it likes to stderr and a great many do —
//! it is read and kept, because "broken pipe" is what a missing binary looks like on the wire
//! and the reason is always on stderr.

use crate::{Cancel, Ops, Output, Tool};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Which revision of MCP this client speaks.
///
/// Sent in `initialize` and compared against what comes back. A server that answers with a
/// different one is not refused — the specification says to negotiate down, and every server in
/// the wild answers with whatever it supports — but the disagreement is worth a line in the log,
/// because a tool that behaves oddly against a newer server is otherwise a mystery.
const PROTOCOL: &str = "2025-06-18";

/// How long a server has to answer one call.
///
/// Generous. An MCP server may be doing real work — a web request, a database query — and the
/// person can interrupt. What this bounds is the case where it has stopped answering at all,
/// which is otherwise a turn that never ends.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(120);

/// The content hash of the program at `command`, as hex.
///
/// **An MCP server is somebody else's code, running as you, with your tools.** pi binds a server
/// to the bytes it was approved as, and it is the one part of MCP's security story a client can
/// actually implement: the protocol has no notion of which server you meant, and a `command` in
/// a config is a name that resolves to whatever is on `$PATH` today.
///
/// Answered rather than enforced here. What is done with it is [`Server::start`]'s business: a
/// declaration that pinned one refuses a mismatch, and one that did not is told the hash so it
/// can pin it. Reporting a hash nobody asked for is what makes pinning a thing a person can
/// start doing, rather than a thing they have to know about first.
///
/// `None` when the program cannot be found or read — which is not a failure here: it is about to
/// fail to start, with a better message than this could give.
#[must_use]
pub fn fingerprint(command: &str) -> Option<String> {
    use sha2::Digest;
    let path = resolve(command)?;
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", sha2::Sha256::digest(&bytes)))
}

/// Where a command name actually resolves, the way `execvp` would.
fn resolve(command: &str) -> Option<std::path::PathBuf> {
    if command.contains(std::path::MAIN_SEPARATOR) {
        let path = std::path::PathBuf::from(command);
        return path.is_file().then_some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
}

/// A running MCP server, and the connection to it.
///
/// One process however many tools it offers, because that is what the protocol is: a server
/// publishes a list and every call names one of them. A process per tool would start the same
/// program five times and give each copy a fifth of the state it keeps.
pub struct Server {
    /// What it was started from, for saying which server a failure belongs to.
    named: String,
    child: Child,
    writer: ChildStdin,
    /// Lines as they arrive, so a read can be given a deadline.
    lines: std::sync::mpsc::Receiver<Result<String, String>>,
    /// Whatever it complained about on the way down.
    ///
    /// A server that fails to start fails on the wire as a closed pipe, which says nothing
    /// anybody can act on. The reason is on its stderr — a missing binary, a bad argument, a
    /// missing credential — and that is the sentence the model and the person need.
    complaint: Arc<Mutex<String>>,
    /// The next JSON-RPC id. Ids must not repeat within a connection.
    next: u64,
    /// What the program hashed to when it was started. See [`fingerprint`].
    fingerprint: Option<String>,
}

impl Server {
    /// Start `command` and complete the handshake.
    ///
    /// # Errors
    /// When the process will not start, or will not answer `initialize`. Both are the same thing
    /// to a caller — there are no tools from this server — and the message says which.
    pub fn start(
        command: &str,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
        pinned: Option<&str>,
    ) -> Result<Self, String> {
        // Before it is started, because after is too late: the point of a pin is that this
        // particular program is the one that was approved to run as you with your tools.
        let fingerprint = fingerprint(command);
        if let Some(pinned) = pinned {
            match &fingerprint {
                Some(actual) if actual == pinned => {}
                Some(actual) => {
                    return Err(format!(
                        "{command} is not the program this configuration pinned: it is {actual} \
                         and the pin says {pinned}. Check what changed, then update the pin"
                    ));
                }
                None => {
                    return Err(format!(
                        "{command} is pinned to {pinned} and cannot be read to check it"
                    ));
                }
            }
        } else if let Some(actual) = &fingerprint {
            // Said, so pinning is something a person can start doing rather than something they
            // have to know about first. `magi doctor` prints it beside the tool.
            magi_model::noted!("mcp: {command} is {actual}");
        }

        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|why| format!("{command} could not be started: {why}"))?;

        let writer = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;

        let complaint = Arc::new(Mutex::new(String::new()));
        {
            let complaint = Arc::clone(&complaint);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if let Ok(mut held) = complaint.lock() {
                        held.push_str(&line);
                        held.push('\n');
                    }
                }
            });
        }

        let (to, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let sent = to.send(line.map_err(|why| why.to_string()));
                if sent.is_err() {
                    return;
                }
            }
        });

        let mut server = Self {
            named: command.to_owned(),
            fingerprint,
            child,
            writer,
            lines,
            complaint,
            next: 0,
        };
        server.handshake()?;
        Ok(server)
    }

    /// `initialize`, then the notification that says the client is ready.
    ///
    /// The notification is not optional. A server that has been initialised and not told so is
    /// entitled to refuse everything after it, and several do.
    fn handshake(&mut self) -> Result<(), String> {
        let said = self.call(
            "initialize",
            serde_json::json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": { "name": "magi", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        if let Some(theirs) = said.get("protocolVersion").and_then(|v| v.as_str())
            && theirs != PROTOCOL
        {
            magi_model::noted!(
                "mcp: {} speaks {theirs} and this client speaks {PROTOCOL}",
                self.named
            );
        }
        self.notify("notifications/initialized", serde_json::json!({}))
    }

    /// What this server offers.
    ///
    /// # Errors
    /// When the server will not answer, or answers something that is not a tool list.
    pub fn tools(&mut self) -> Result<Vec<Declared>, String> {
        let said = self.call("tools/list", serde_json::json!({}))?;
        let listed = said
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("{}: tools/list answered no list", self.named))?;
        Ok(listed
            .iter()
            .filter_map(|tool| {
                Some(Declared {
                    name: tool.get("name")?.as_str()?.to_owned(),
                    description: tool
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    // MCP calls it `inputSchema`; everything here calls it `parameters`, and the
                    // registry checks a call against it like any other.
                    parameters: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
                })
            })
            .collect())
    }

    /// Run one tool.
    ///
    /// # Errors
    /// When the server will not answer. A tool that ran and *failed* is not an error here: it
    /// comes back as an [`Output`] the model reads, which is the same rule every other transport
    /// follows.
    pub fn run(&mut self, name: &str, arguments: &serde_json::Value) -> Result<Output, String> {
        let said = self.call(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )?;
        Ok(Output {
            content: text_of(&said),
            is_error: said
                .get("isError")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            shown: None,
        })
    }

    /// One request, and the result it answered.
    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.next += 1;
        let id = self.next;
        self.write(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))?;

        // Until the answer to *this* id. A server may write notifications and progress at any
        // time, and a client that took the next line as its answer would read one of those.
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(Ok(line)) => line,
                Ok(Err(why)) => return Err(self.blame(&format!("{method}: {why}"))),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Err(self.blame(&format!("{method}: no answer in {PATIENCE:?}")));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.blame(&format!("{method}: the server ended")));
                }
            };
            let Ok(said) = serde_json::from_str::<serde_json::Value>(&line) else {
                // Not JSON at all. Servers do print banners to stdout, wrongly, and dying on one
                // would make a working server unusable over a line nobody reads.
                magi_model::noted!(
                    "mcp: {} wrote a line that is not JSON: {line:.120}",
                    self.named
                );
                continue;
            };
            if said.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(wrong) = said.get("error") {
                let why = wrong
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("no reason given");
                return Err(format!("{}: {method}: {why}", self.named));
            }
            return Ok(said
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
    }

    /// A notification, which has no id and expects no answer.
    fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), String> {
        self.write(serde_json::json!({
            "jsonrpc": "2.0", "method": method, "params": params,
        }))
    }

    /// One object, one line.
    fn write(&mut self, body: serde_json::Value) -> Result<(), String> {
        writeln!(self.writer, "{body}")
            .and_then(|()| self.writer.flush())
            .map_err(|why| self.blame(&why.to_string()))
    }

    /// A failure, with whatever the server said about itself attached.
    fn blame(&self, why: &str) -> String {
        let said = self
            .complaint
            .lock()
            .map(|held| held.trim().to_owned())
            .unwrap_or_default();
        if said.is_empty() {
            format!("{}: {why}", self.named)
        } else {
            format!("{}: {why}\n{said}", self.named)
        }
    }
}

impl Drop for Server {
    /// Close the pipe, which is how an MCP server is asked to stop, then reap it.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What a server said about one of its tools.
#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    /// The name the model calls it by.
    pub name: String,
    /// What it does, in the model's terms.
    pub description: String,
    /// JSON Schema for its arguments.
    pub parameters: serde_json::Value,
}

/// The text of an MCP result, as the model should read it.
///
/// MCP answers content as a list of typed parts. Only `text` is carried through: an image or a
/// resource has no place in a transcript that goes to a provider as text, and saying so plainly
/// beats passing base64 the model cannot use.
fn text_of(result: &serde_json::Value) -> String {
    let Some(parts) = result.get("content").and_then(|v| v.as_array()) else {
        return String::new();
    };
    parts
        .iter()
        .map(
            |part| match part.get("type").and_then(serde_json::Value::as_str) {
                Some("text") => part
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                Some(other) => {
                    format!("[{other} content, which magi does not carry into a transcript]")
                }
                None => String::new(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n")
}

/// One tool from an MCP server.
///
/// Holds the server rather than a copy of it: several tools come from one process, and each of
/// them calling it is the protocol working as designed.
pub struct McpTool {
    server: Rc<RefCell<Server>>,
    declared: Declared,
}

impl McpTool {
    /// Every tool a server offers, ready to register.
    ///
    /// # Errors
    /// When the server will not start or will not list. Both mean no tools from it, and the
    /// message says which — a session carries on without them, as it does without casper.
    pub fn all(
        command: &str,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
        pinned: Option<&str>,
    ) -> Result<Vec<Self>, String> {
        let mut server = Server::start(command, args, env, pinned)?;
        let declared = server.tools()?;
        let server = Rc::new(RefCell::new(server));
        Ok(declared
            .into_iter()
            .map(|declared| Self {
                server: Rc::clone(&server),
                declared,
            })
            .collect())
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.declared.name
    }

    fn description(&self) -> &str {
        &self.declared.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.declared.parameters.clone()
    }

    fn composition(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("transport", "mcp".to_owned())];
        if let Ok(server) = self.server.try_borrow() {
            out.push(("server", server.named.clone()));
            if let Some(fingerprint) = &server.fingerprint {
                // Printed so it can be copied into the declaration's `sha256`, which is the
                // whole of how pinning gets started.
                out.push(("sha256", fingerprint.clone()));
            }
        }
        out
    }

    fn run(&self, arguments: &serde_json::Value, _ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        // A borrow that is already taken means one MCP tool called another, which the turn loop
        // does not do: calls are prepared one at a time and run in order.
        let Ok(mut server) = self.server.try_borrow_mut() else {
            return Output::error("this MCP server is already answering another call");
        };
        match server.run(&self.declared.name, arguments) {
            Ok(output) => output,
            Err(why) => Output::error(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Declared, McpTool, Server, text_of};
    use crate::{Registry, Tool, Uncancelled, ops::Real};
    use magi_model::scratch::Scratch;

    /// A server that speaks exactly as much of MCP as a client needs.
    ///
    /// Written here rather than mocked, because what is under test is the *protocol*: the
    /// handshake a server may refuse everything without, the notification that is not optional,
    /// the id matching that a notification arriving mid-call would break. A mock would agree
    /// with whatever this client happens to do.
    const SERVER: &str = r#"
import sys, json
TOOLS = [{"name": "echo", "description": "Echo what it was given.",
          "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}},
                          "required": ["text"]}}]
ready = False
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    req = json.loads(line)
    m, i = req.get("method"), req.get("id")
    if m == "notifications/initialized":
        ready = True
        continue
    if i is None: continue
    if m == "initialize":
        out = {"protocolVersion": "2025-06-18", "serverInfo": {"name": "fake", "version": "1"}}
    elif not ready:
        print(json.dumps({"jsonrpc":"2.0","id":i,
              "error":{"code":-32002,"message":"not initialised"}}), flush=True)
        continue
    elif m == "tools/list":
        out = {"tools": TOOLS}
    elif m == "tools/call":
        args = req.get("params", {}).get("arguments", {})
        # A notification first, which a client that read the next line as its answer would
        # mistake for one.
        print(json.dumps({"jsonrpc":"2.0","method":"notifications/progress",
                          "params":{"progress":1}}), flush=True)
        out = {"content": [{"type": "text", "text": "you said: " + str(args.get("text",""))},
                           {"type": "image", "data": "…"}],
               "isError": False}
    else:
        print(json.dumps({"jsonrpc":"2.0","id":i,
              "error":{"code":-32601,"message":"no such method"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": i, "result": out}), flush=True)
"#;

    /// The fake server on disk, and the command that runs it.
    fn serving(name: &str) -> (Scratch, String, Vec<String>) {
        let dir = Scratch::new("magi-mcp", name);
        let at = dir.join("server.py");
        std::fs::write(&at, SERVER).expect("write");
        (dir, "python3".to_owned(), vec![at.display().to_string()])
    }

    #[test]
    fn a_server_is_asked_what_it_offers_and_answers() {
        let (_dir, command, args) = serving("list");
        let tools = McpTool::all(&command, &args, &Default::default(), None).expect("it starts");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "echo");
        assert_eq!(tools[0].description(), "Echo what it was given.");
        // MCP calls it `inputSchema` and everything here calls it `parameters`; the registry
        // checks a call against it like any other, so the rename has to happen at the boundary.
        assert_eq!(
            tools[0].parameters()["properties"]["text"]["type"],
            "string"
        );
    }

    #[test]
    fn a_tool_runs_and_its_answer_comes_back_as_text() {
        let (_dir, command, args) = serving("call");
        let tools = McpTool::all(&command, &args, &Default::default(), None).expect("it starts");
        let out = tools[0].run(
            &serde_json::json!({ "text": "hello" }),
            &Real::new(std::env::temp_dir()),
            &Uncancelled,
        );
        assert!(!out.is_error, "{out:?}");
        assert!(out.content.contains("you said: hello"), "{}", out.content);
        // And the part that is not text says so rather than arriving as base64 nobody can read.
        assert!(out.content.contains("image content"), "{}", out.content);
    }

    #[test]
    fn a_notification_arriving_mid_call_is_not_mistaken_for_the_answer() {
        // The server writes a progress notification before every result. A client that took the
        // next line as its answer would read that one — and the failure is a tool that returns
        // nothing, intermittently, against servers that report progress.
        let (_dir, command, args) = serving("progress");
        let tools = McpTool::all(&command, &args, &Default::default(), None).expect("it starts");
        for _ in 0..3 {
            let out = tools[0].run(
                &serde_json::json!({ "text": "again" }),
                &Real::new(std::env::temp_dir()),
                &Uncancelled,
            );
            assert!(out.content.contains("you said: again"), "{}", out.content);
        }
    }

    #[test]
    fn the_initialized_notification_is_not_optional() {
        // A server that has been initialised and not told so is entitled to refuse everything
        // after it, and several do. This one does, which is why `tools/list` works at all above.
        let (_dir, command, args) = serving("handshake");
        let mut server = Server::start(&command, &args, &Default::default(), None)
            .expect("the handshake completes");
        assert!(
            server.tools().is_ok(),
            "the server refused everything after initialize"
        );
    }

    #[test]
    fn an_mcp_tool_registers_beside_every_other_kind() {
        // The whole design claim: a transport is a property of a declaration, not a second
        // registry. It is checked against the same schema, capped by the same bound, and the
        // turn loop does not know MCP exists.
        let (_dir, command, args) = serving("registry");
        let mut registry = Registry::new();
        crate::builtin::install(&mut registry);
        for tool in McpTool::all(&command, &args, &Default::default(), None).expect("it starts") {
            registry.register(Box::new(tool));
        }
        let ops = Real::new(std::env::temp_dir());

        // Refused by the registry's own schema check, before the server is even asked.
        let wrong = registry.call("echo", &serde_json::json!({}), &ops, &Uncancelled);
        assert!(wrong.is_error, "{wrong:?}");
        assert!(wrong.content.contains("do not fit"), "{}", wrong.content);

        let right = registry.call(
            "echo",
            &serde_json::json!({ "text": "through the registry" }),
            &ops,
            &Uncancelled,
        );
        assert!(!right.is_error, "{right:?}");
        assert!(right.content.contains("through the registry"));
    }

    #[test]
    fn a_pinned_server_that_is_not_the_pinned_program_refuses_to_start() {
        // **An MCP server is somebody else's code, running as you, with your tools**, and the
        // `command` in a config is a name that resolves to whatever is on `$PATH` today. This is
        // the one part of MCP's security story a client can implement: the protocol has no
        // notion of *which* server you meant.
        let (_dir, command, args) = serving("pinned");
        let actual = super::fingerprint(&command).expect("python3 is readable");

        let Err(why) = McpTool::all(
            &command,
            &args,
            &Default::default(),
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
        ) else {
            panic!("a program that is not the pinned one started anyway");
        };
        assert!(why.contains("pinned"), "{why}");
        assert!(why.contains(&actual), "it says what it actually is: {why}");

        // And the right pin starts, so pinning is usable rather than merely refusable.
        let tools = McpTool::all(&command, &args, &Default::default(), Some(&actual))
            .expect("the pinned program is this one");
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn a_server_says_what_it_hashed_to_so_a_pin_can_be_written() {
        // How pinning gets started: `magi doctor` prints this beside the tool, and a person
        // copies it into the declaration. A feature nobody can discover is one nobody uses.
        let (_dir, command, args) = serving("fingerprint");
        let tools = McpTool::all(&command, &args, &Default::default(), None).expect("it starts");
        let said: std::collections::BTreeMap<_, _> = tools[0].composition().into_iter().collect();
        assert_eq!(said.get("transport").map(String::as_str), Some("mcp"));
        assert_eq!(
            said.get("sha256"),
            super::fingerprint(&command).as_ref(),
            "what it hashed to, ready to be pinned"
        );
    }

    #[test]
    fn a_program_that_is_not_there_hashes_to_nothing() {
        assert_eq!(super::fingerprint("magi-no-such-program-at-all"), None);
    }

    #[test]
    fn a_server_that_will_not_start_says_why_rather_than_broken_pipe() {
        let Err(why) = McpTool::all("magi-no-such-mcp-server", &[], &Default::default(), None)
        else {
            panic!("a server that is not installed cannot start");
        };
        assert!(why.contains("could not be started"), "{why}");
    }

    #[test]
    fn only_text_is_carried_into_a_transcript() {
        let said = text_of(&serde_json::json!({
            "content": [
                { "type": "text", "text": "one" },
                { "type": "resource", "uri": "file:///x" },
                { "type": "text", "text": "two" },
            ]
        }));
        assert!(said.contains("one") && said.contains("two"));
        assert!(said.contains("resource content"), "{said}");
    }

    #[test]
    fn a_declaration_is_what_the_server_said_it_was() {
        let declared = Declared {
            name: "x".to_owned(),
            description: String::new(),
            parameters: serde_json::json!({ "type": "object" }),
        };
        assert_eq!(declared.parameters["type"], "object");
    }
}
