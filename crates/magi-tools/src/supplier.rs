//! Tools supplied by the program filling the `tools` role — casper, unless `magi.tools` names
//! another. magi has no tools of its own; every one is reached by asking this program what exists,
//! handing over a call and reading back what it produced.
//!
//! Two doors, chosen by `MAGI_TOOLS_DOOR` and mirroring the agent and memory switches. `command`,
//! the default, spawns the program once per call. `library` connects to one `serve` the program
//! keeps running for the session: the jail is the same either way, because it is the one magi gave
//! `serve` at spawn, not one the call named. [`magi_proto::tooling::Ran`] carries `said` for the
//! model and `shown` for the screen; this module keeps `said`, because a [`Tool`] returns text.

use crate::holding::{CONFIGURE, CONFIGURE_WAS};
use crate::question::Asks;
use crate::{Cancel, Ops, Output, Tool, Uncancelled};
use magi_proto::tooling::{Call, Card, Ran, Shown, Wondering};
use magi_proto::wondering::Answered;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

mod admission;
mod dispatch;
mod failure;
pub mod launch;
mod mcp;
mod transport;

#[cfg(test)]
mod containment;
mod door;
mod waiting;
#[cfg(test)]
mod wondering;

#[cfg(test)]
fn fixture_reader() -> String {
    let path = std::env::split_paths(&std::env::var_os("PATH").expect("fixture PATH"))
        .map(|dir| dir.join("cat"))
        .find(|path| path.is_file())
        .expect("fixture cat executable");
    let path = std::path::absolute(path).expect("absolute fixture utility");
    magi_proto::permit::quoted(path.to_str().expect("UTF-8 fixture utility"))
}

/// The newest revision of the family's reply shape this build reads.
const FAMILY: u64 = 1;

/// How long `program tools` may take to list what it offers.
const LISTING: Duration = Duration::from_secs(30);

/// The program that fills the `tools` role when no configuration names one, found on `PATH`.
pub const CASPER: &str = "casper";

/// Which door magi reaches the tools program on: `command` (the default) spawns one process per
/// call; `library` connects to one `serve` it keeps running for the session. Mirrors the agent and
/// memory door switches.
pub const DOOR: &str = "MAGI_TOOLS_DOOR";

/// Close this process's daemon registry and reap every retained native daemon.
pub fn shutdown() {
    door::shutdown();
}

/// The program filling the `tools` role, and what this session tells it.
///
/// One value, so the program name reaches every spawn the pin and the settings reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tooling {
    /// As `magi.tools` named it, or [`CASPER`] when a configuration named nobody.
    pub program: String,
    /// The SHA-256 that program must hash to, if this configuration pinned one.
    pub pin: Option<String>,
    /// What this session tells it to be, as the JSON it goes over. One process per call, so the
    /// settings ride on every spawn rather than being sent once. Empty means "whatever it is".
    pub configure: String,
    /// The roles the configuration describes, which `spawn` offers a child by name.
    pub kinds: Vec<crate::builtin::Kind>,
}

impl Default for Tooling {
    /// casper, unpinned, unconfigured: what a session had before any of this was nameable.
    fn default() -> Self {
        Self {
            program: CASPER.to_owned(),
            pin: None,
            configure: String::new(),
            kinds: Vec::new(),
        }
    }
}

impl Tooling {
    /// The role filled by a named program, with nothing else said about it.
    #[must_use]
    pub fn of(program: &str) -> Self {
        Self {
            program: program.to_owned(),
            ..Self::default()
        }
    }
}

/// What `program` says it offers. Empty when it is not installed or would not answer, which is not
/// an error: the session keeps the tools magi declares itself.
#[must_use]
pub fn cards_from(program: &str) -> Vec<Card> {
    cards_configured(program, "")
}

/// The same, saying what this session has configured the program to be. It is one process per
/// call, so the configuration goes on every spawn. Empty means "whatever it is by default".
#[must_use]
pub fn cards_configured(program: &str, configured: &str) -> Vec<Card> {
    cards_environment(program, configured, &std::collections::BTreeMap::new())
}

/// The same, with this session's environment overlay on the spawn.
fn cards_environment(
    program: &str,
    configured: &str,
    environ: &std::collections::BTreeMap<String, String>,
) -> Vec<Card> {
    let mut command = std::process::Command::new(program);
    command
        .envs(environ)
        .arg("tools")
        .env(CONFIGURE, configured)
        .env(CONFIGURE_WAS, configured);
    match waiting::output(command, None, &Uncancelled, LISTING) {
        Ok(out) => listed(rows(&out).unwrap_or_default()),
        Err(why) => {
            magi_model::noted!("tools: {program} tools: {why}");
            Vec::new()
        }
    }
}

/// The cards in a reply's rows, whichever of the two shapes they arrived in. Flat is the contract;
/// the nested shape casper also sends is read too, because the two ship from separate repositories.
fn listed(rows: Vec<serde_json::Value>) -> Vec<Card> {
    if let Some(nested) = rows.first().filter(|first| first.is_array()) {
        return serde_json::from_value(nested.clone()).unwrap_or_default();
    }
    rows.into_iter()
        .filter_map(|row| serde_json::from_value(row).ok())
        .collect()
}

/// The rows of a family reply, or why there are none: not the family's shape, written in a newer
/// `family`, a refusal in the program's own words, or an `n` that disagrees with what came.
fn rows(body: &[u8]) -> Result<Vec<serde_json::Value>, String> {
    rows_known(body).map_err(failure::Failure::into_string)
}

fn rows_known(body: &[u8]) -> Result<Vec<serde_json::Value>, failure::Failure> {
    use failure::Failure;
    let reply: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| Failure::unknown("answered something that is not the family's reply"))?;
    if let Some(family) = reply.get("family")
        && family.as_u64().is_none_or(|revision| revision > FAMILY)
    {
        return Err(Failure::unknown(format!(
            "answered in family {family}, newer than this magi reads"
        )));
    }
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let error = reply
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("refused without saying why");
        let fault = reply
            .get("fault")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("refused");
        let reason = format!("{fault}: {error}");
        return Err(
            if reply.get("ok").and_then(serde_json::Value::as_bool) == Some(false)
                && reply.get("family").and_then(serde_json::Value::as_u64) == Some(FAMILY)
                && reply.get("n").and_then(serde_json::Value::as_u64) == Some(0)
                && reply
                    .get("result")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(Vec::is_empty)
                && reply.get("fault").and_then(serde_json::Value::as_str) == Some("refused")
                && reply
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|error| !error.is_empty() && error.len() <= 4096)
            {
                Failure::refused(reason)
            } else {
                Failure::unknown(reason)
            },
        );
    }
    let result = reply
        .get("result")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Failure::unknown("answered with no result"))?;
    match reply.get("n").map(serde_json::Value::as_u64) {
        None => Ok(result.clone()),
        Some(Some(n)) if usize::try_from(n).is_ok_and(|n| n == result.len()) => Ok(result.clone()),
        Some(_) => Err(Failure::unknown(format!(
            "said it answered {} rows and sent {}",
            reply.get("n").unwrap_or(&serde_json::Value::Null),
            result.len()
        ))),
    }
}

#[cfg(test)]
mod failures;

/// Hand one call to `program` and read back what it produced.
///
/// # Errors
/// A refusal — the program could not be started, or would not take the call. Distinct from a tool
/// that *ran* and reported a problem, which comes back as [`Ran::failed`].
pub fn run(program: &str, call: &Call) -> Result<Ran, String> {
    run_configured(program, call, "")
}

/// The same, saying what this session has configured the program to be. See [`cards_configured`].
///
/// # Errors
/// A refusal — the program could not be started, or would not take the call.
pub fn run_configured(program: &str, call: &Call, configured: &str) -> Result<Ran, String> {
    run_jailed(program, call, configured, None, &Uncancelled)
}

/// The same, inside the jail `jail` describes when `Some`, carried in [`crate::holding::JAIL`],
/// and stopped when `cancel` says so.
///
/// # Errors
/// A refusal — the program could not be started, would not take the call, or was stopped.
pub fn run_jailed(
    program: &str,
    call: &Call,
    configured: &str,
    jail: Option<&str>,
    cancel: &dyn Cancel,
) -> Result<Ran, String> {
    let context = crate::holding::Context {
        environ: Default::default(),
        configure: configured.to_owned(),
        jail: jail.map(str::to_owned),
        cwd: (!call.cwd.is_empty()).then(|| PathBuf::from(&call.cwd)),
        call: None,
    };
    run_context(program, call, &context, cancel)
}

/// Run a supplied tool with its session environment and containment settings.
///
/// # Errors
/// As [`run_jailed`].
pub fn run_context(
    program: &str,
    call: &Call,
    context: &crate::holding::Context,
    cancel: &dyn Cancel,
) -> Result<Ran, String> {
    transport::run(program, call, context, cancel).map_err(failure::Failure::into_string)
}

/// One tool the role's program supplied, as magi's registry sees it.
pub struct SuppliedTool {
    /// This session's environment overlay, carried on every spawn.
    environ: std::collections::BTreeMap<String, String>,
    card: Card,
    program: String,
    asks: Arc<dyn Asks>,
    holds: Arc<dyn crate::holding::Holds>,
    /// What answers a question the tool puts to magi rather than to the person.
    knows: Arc<dyn crate::holding::Answers>,
    /// What this session told the program to be, carried on every spawn. See [`cards_configured`].
    configured: String,
}

impl SuppliedTool {
    /// Every tool the role's program offers, if it is the program that was pinned. It supplies
    /// magi's tool set and is resolved off `$PATH`, so one earlier on the path owns `shell`, `read`
    /// and everything else the model calls. No pin starts anything, which is the ordinary case;
    /// `magi doctor` prints what the program actually hashed to.
    #[must_use]
    pub fn pinned(
        tooling: &Tooling,
        asks: Arc<dyn Asks>,
        holds: Arc<dyn crate::holding::Holds>,
        knows: Arc<dyn crate::holding::Answers>,
        environ: &std::collections::BTreeMap<String, String>,
    ) -> Vec<Self> {
        let program = match verified(tooling) {
            Ok(program) => program,
            Err(why) => {
                eprintln!("magi: {why}. No tools were taken from it");
                return Vec::new();
            }
        };
        let tooling = &Tooling {
            program,
            ..tooling.clone()
        };
        Self::all(tooling, asks, holds, knows, environ)
    }

    /// Every tool the role's program offers, ready to register. `asks` is how a question reaches
    /// the person, taken here rather than looked up when the question arrives.
    #[must_use]
    pub fn all(
        tooling: &Tooling,
        asks: Arc<dyn Asks>,
        holds: Arc<dyn crate::holding::Holds>,
        knows: Arc<dyn crate::holding::Answers>,
        environ: &std::collections::BTreeMap<String, String>,
    ) -> Vec<Self> {
        let cards = cards_environment(&tooling.program, &tooling.configure, environ);
        let configured = mcp::with_catalog(&tooling.configure, &cards);
        cards
            .into_iter()
            .map(|card| Self {
                environ: environ.clone(),
                card,
                program: tooling.program.clone(),
                asks: Arc::clone(&asks),
                holds: Arc::clone(&holds),
                knows: Arc::clone(&knows),
                configured: configured.clone(),
            })
            .collect()
    }
}

impl Tool for SuppliedTool {
    fn file_scope(&self) -> Option<String> {
        use sha2::{Digest, Sha256};
        self.card
            .file_revision
            .as_ref()
            .filter(|capability| capability.version == 1)?;
        let bytes = serde_json::to_vec(&(&self.program, &self.configured, &self.environ)).ok()?;
        Some(format!("{:x}", Sha256::digest(bytes)))
    }

    fn composition(&self) -> Vec<(&'static str, String)> {
        // The program's own name, not the role's default.
        let mut out = vec![
            ("transport", self.program.clone()),
            (
                "command",
                format!("{} run {}", self.program, self.card.name),
            ),
        ];
        // Printed so it can be pinned: this program supplies the whole tool set and is found on
        // `$PATH`.
        if let Some(fingerprint) = fingerprint(&self.program) {
            out.push(("sha256", fingerprint));
        }
        out
    }

    fn name(&self) -> &str {
        &self.card.name
    }

    fn description(&self) -> &str {
        &self.card.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.card.parameters.clone()
    }

    fn deferred(&self) -> bool {
        self.card.deferred
    }

    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops, cancel: &dyn Cancel) -> Output {
        dispatch::run(self, arguments, ops, cancel)
    }
}

/// Answer one [`Wondering`], as the JSON the call resumes with. A verb magi does not know is
/// refused here rather than guessed at, and a refusal reads the same either way.
fn answered(knows: &Arc<dyn crate::holding::Answers>, wondering: &Wondering) -> String {
    let answer = magi_proto::wondering::EVERY
        .iter()
        .find(|wonder| wonder.verb() == wondering.wonder)
        .map_or_else(
            || Answered::Refused {
                because: format!("no such question: {}", wondering.wonder),
            },
            |wonder| knows.answer(*wonder, &wondering.args),
        );
    let said = match answer {
        Answered::Told { said } => serde_json::json!({ "told": said }),
        Answered::Refused { because } => serde_json::json!({ "refused": because }),
    };
    said.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn program_fingerprints_stream_bounded_chunks_and_preserve_exact_bytes() {
        use sha2::Digest;
        struct Chunks {
            left: usize,
            interrupted: bool,
        }
        impl std::io::Read for Chunks {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                assert!(output.len() <= 64 * 1024);
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                let count = self.left.min(output.len()).min(997);
                output[..count].fill(0xa5);
                self.left -= count;
                Ok(count)
            }
        }
        let size = 3 * 1024 * 1024 + 17;
        let expected = format!("{:x}", sha2::Sha256::digest(vec![0xa5; size]));
        assert_eq!(
            super::fingerprint_reader(Chunks {
                left: size,
                interrupted: false
            }),
            Some(expected)
        );
        assert_eq!(
            super::fingerprint_reader(&b""[..]),
            Some(format!("{:x}", sha2::Sha256::digest([])))
        );
    }

    /// A pinned casper that is not the pinned program supplies nothing.
    #[test]
    fn a_pinned_casper_that_is_not_the_pinned_program_supplies_no_tools() {
        let asks: Arc<dyn Asks> = Arc::new(crate::question::Unanswered);
        let holds: Arc<dyn crate::holding::Holds> = Arc::new(crate::holding::Screenless);
        let knows: Arc<dyn crate::holding::Answers> = Arc::new(crate::holding::Incurious);

        let wrong = SuppliedTool::pinned(
            &Tooling {
                pin: Some(
                    "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
                ),
                ..Tooling::default()
            },
            Arc::clone(&asks),
            Arc::clone(&holds),
            Arc::clone(&knows),
            &Default::default(),
        );
        assert!(wrong.is_empty(), "a substituted casper supplied tools");

        // Skipped when casper is not installed, which is a session with no tools from it either way.
        if let Some(actual) = fingerprint(CASPER) {
            let right = SuppliedTool::pinned(
                &Tooling {
                    pin: Some(actual),
                    ..Tooling::default()
                },
                asks,
                holds,
                knows,
                &Default::default(),
            );
            assert_eq!(
                right.len(),
                SuppliedTool::all(
                    &Tooling::default(),
                    Arc::new(crate::question::Unanswered),
                    Arc::new(crate::holding::Screenless),
                    Arc::new(crate::holding::Incurious),
                    &Default::default(),
                )
                .len(),
                "pinning the right program changed what it offers"
            );
        }
    }

    use super::*;

    #[test]
    fn a_program_is_verified_against_its_pin_at_the_one_path_it_resolves_to() {
        use sha2::Digest;
        use std::os::unix::fs::PermissionsExt;
        let dir = magi_model::scratch::Scratch::new("magi-verified", "pin");
        let program = dir.join("tools");
        std::fs::write(&program, "#!/bin/sh\n").expect("write");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
        let linked = dir.join("linked");
        std::os::unix::fs::symlink(&program, &linked).expect("link");
        let real = program.canonicalize().expect("real").display().to_string();
        let pin = format!(
            "{:x}",
            sha2::Sha256::digest(std::fs::read(&program).expect("bytes"))
        );
        let named = |pin: Option<String>| Tooling {
            program: linked.display().to_string(),
            pin,
            ..Tooling::default()
        };
        assert_eq!(verified(&named(None)), Ok(real.clone()));
        assert_eq!(verified(&named(Some(pin))), Ok(real));
        let why = verified(&named(Some("0".repeat(64)))).expect_err("another program");
        assert!(
            why.contains("not the program this configuration pinned"),
            "{why}"
        );
        let gone = Tooling {
            program: dir.join("absent").display().to_string(),
            pin: Some("0".repeat(64)),
            ..Tooling::default()
        };
        assert!(verified(&gone).is_err(), "nothing there to check");
    }

    #[test]
    fn a_casper_that_is_not_there_offers_nothing_rather_than_failing() {
        // The ordinary case on a machine without it: the session keeps the tools magi declares.
        assert!(cards_from("magi-no-such-casper-anywhere").is_empty());
    }

    #[test]
    fn a_call_to_something_absent_says_which_program() {
        let why = run(
            "magi-no-such-casper-anywhere",
            &Call {
                tool: "ls".to_owned(),
                args: serde_json::Value::Null,
                cwd: String::new(),
                answered: None,
                revision: Default::default(),
            },
        )
        .expect_err("nothing to call");
        assert!(why.contains("magi-no-such-casper-anywhere"), "{why}");
    }

    #[test]
    fn a_reply_that_is_not_the_familys_shape_yields_nothing() {
        assert!(rows(b"not json at all").is_err());
        assert_eq!(
            rows(br#"{"ok":true,"n":1,"result":[1]}"#).map(|r| r.len()),
            Ok(1)
        );
    }

    #[test]
    fn rows_honour_family_n_and_fault_and_say_the_refusal() {
        let why = rows(br#"{"ok":false,"fault":"failed","error":"the jail would not start"}"#)
            .expect_err("refused");
        assert!(
            why.contains("the jail would not start") && why.contains("failed"),
            "{why}"
        );
        assert!(rows(br#"{"ok":false}"#).is_err());
        let newer = rows(br#"{"ok":true,"family":2,"n":1,"result":[1]}"#).expect_err("newer");
        assert!(newer.contains("family 2"), "{newer}");
        assert!(rows(br#"{"ok":true,"family":1,"n":1,"result":[1]}"#).is_ok());
        assert!(
            rows(br#"{"ok":true,"result":[1]}"#).is_ok(),
            "before `n` and `family` existed"
        );
        assert!(rows(br#"{"ok":true,"n":2,"result":[1]}"#).is_err());
        assert!(rows(br#"{"ok":true,"n":"one","result":[1]}"#).is_err());
    }

    #[test]
    fn cards_are_read_in_either_shape_casper_has_sent_them() {
        // Flat is the contract; nested is what casper sent for as long as it has existed. The two
        // programs are installed separately and either side can be upgraded first.
        let card =
            serde_json::json!({ "name": "cat", "description": "read a file", "parameters": {} });
        let flat = listed(vec![card.clone()]);
        let nested = listed(vec![serde_json::json!([card])]);
        assert_eq!(flat.len(), 1, "flat");
        assert_eq!(nested.len(), 1, "nested");
        assert_eq!(flat[0].name, nested[0].name);
    }
}

/// One finished call, as the registry wants it: `said` for the model, `shown` for the screen. A
/// tool that reported a problem is still a result, so a failure carries its view too.
fn finished(ran: Ran) -> Output {
    let files = (ran.files != Default::default()).then(|| Box::new(ran.files));
    Output {
        images: ran.images,
        content: ran.said,
        is_error: ran.failed,
        shown: ran.shown,
        unlocks: ran.unlocks,
        hints: ran.hints,
        files,
        file_delivery: None,
    }
}

/// What this call is about to do, in magi's own vocabulary. `None` when the card names no verb — a
/// tool that touches nothing a person would want a say over is not gated. A verb this build has no
/// meaning for is treated as `run`, the most guarded thing there is, rather than waved through.
fn wants(card: &Card, arguments: &serde_json::Value) -> Option<magi_proto::permit::Action> {
    use magi_proto::permit::Action;
    let needs = card.needs.as_deref()?;
    if let Some(action) = mcp::permission(card) {
        return Some(action);
    }
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    // A tool with no local path, or one whose path is an MCP server's, is asked about by its own
    // name and never resolved under the session's directory.
    let by_name = || Action::Run {
        command: card.name.clone(),
        program: card.name.clone(),
    };
    let path = text("path");
    let local = !path.is_empty() && !card.name.starts_with("mcp__");
    Some(match needs {
        "read" if local => Action::Read { path },
        "write" if local => Action::Write { path },
        "read" | "write" => by_name(),
        "reach" => Action::Network {
            host: host_of(&text("host"), &text("url")),
        },
        _ if card.name.starts_with("mcp__") => by_name(),
        _ => {
            let command = {
                let command = text("command");
                if command.is_empty() {
                    card.name.clone()
                } else {
                    command
                }
            };
            // The same reading the process transport uses. Taking the first word outright made
            // `FOO=1 git status` offer "any `FOO=1` command", whose grant then covered every
            // command line starting `FOO=1`. A permission subject that differs by transport is a bug.
            let program = first_word(&command);
            let program = if program.is_empty() {
                card.name.clone()
            } else {
                program
            };
            Action::Run { command, program }
        }
    })
}

/// The host a `reach` call names: `host` when it says one, else the host part of `url`, lowercased
/// and without credentials or port. Empty when neither names one.
fn host_of(host: &str, url: &str) -> String {
    if !host.trim().is_empty() {
        return host.trim().to_lowercase();
    }
    let rest = url
        .trim()
        .split_once("://")
        .map_or(url.trim(), |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, at)| at);
    let name = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    name.to_lowercase()
}

/// The command's program name, for the permission subject, stepping over any leading `VAR=value`.
fn first_word(command: &str) -> String {
    command
        .split_whitespace()
        .find(|word| !word.contains('=') || word.starts_with('/'))
        .unwrap_or("")
        .to_owned()
}

/// The SHA-256 of the tools program on disk, for pinning: a coordinator records it and a mismatch
/// on the next run is a different binary answering. `None` when the program cannot be found or read.
fn fingerprint(command: &str) -> Option<String> {
    let path = resolve(command)?;
    fingerprint_reader(std::fs::File::open(path).ok()?)
}

fn fingerprint_reader(mut reader: impl std::io::Read) -> Option<String> {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Some(format!("{:x}", hash.finalize())),
            Ok(count) => hash.update(buffer.get(..count)?),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
}

/// Where a command name resolves, the way `execvp` would: an executable regular file, found on
/// the absolute directories of `PATH` when the name has no slash, and resolved through links.
fn resolve(command: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let runnable = |path: &std::path::Path| {
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    let found = if command.contains(std::path::MAIN_SEPARATOR) {
        Some(std::path::PathBuf::from(command)).filter(|path| runnable(path))
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(command))
            .find(|candidate| runnable(candidate))
    };
    found?.canonicalize().ok()
}

/// The tools program as one absolute path, resolved once, so the bytes a pin is checked against
/// are the bytes every call runs. The name as given when it resolves to nothing.
#[must_use]
pub fn located(program: &str) -> String {
    resolve(program).map_or_else(|| program.to_owned(), |path| path.display().to_string())
}

/// The tools program as one absolute path, when it is the program the configuration pinned: what
/// is run for the session's tools and for anything else it asks of that program.
///
/// # Errors
/// When a pin was set and the program does not hash to it, or cannot be read to check.
pub fn verified(tooling: &Tooling) -> Result<String, String> {
    let program = located(&tooling.program);
    let Some(pinned) = &tooling.pin else {
        return Ok(program);
    };
    match fingerprint(&program) {
        Some(actual) if &actual == pinned => Ok(program),
        Some(actual) => Err(format!(
            "{program} is not the program this configuration pinned: it is {actual} and the pin \
             says {pinned}"
        )),
        None => Err(format!(
            "{program} is pinned to {pinned} and cannot be read to check"
        )),
    }
}

#[cfg(test)]
mod gating;
