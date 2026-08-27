//! A checkable definition of what a tool peer must do.
//!
//! Two peers written by the same people, in the same language, against the same codec cannot
//! disagree with the host — so they cannot show that the protocol is written down anywhere
//! except in the code that speaks it. This can: it drives any program through the exchanges a
//! peer has to survive and says what it got wrong, in terms someone reimplementing the wire
//! format from the documentation can act on.
//!
//! Exported rather than kept in a test file so a peer written elsewhere can be checked by its
//! own author. That is the difference between a protocol and an internal calling convention.

use axum_ipc::blocking::{FrameReader, FrameWriter};
use axum_proto::{ToolCallId, ToolReport, ToolRequest};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// How long any single exchange is given.
const PATIENCE: Duration = Duration::from_secs(10);

/// Something a peer did that the protocol does not allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The rule that was broken, as a peer author would look it up.
    pub rule: &'static str,
    /// What happened instead.
    pub detail: String,
}

/// What a peer must be told in order to be checked.
pub struct Subject<'a> {
    /// The program to run.
    pub command: &'a str,
    /// Its arguments.
    pub args: &'a [String],
    /// Where to run it.
    pub dir: &'a Path,
    /// A call the peer should be able to answer, as `(tool, arguments)`.
    ///
    /// Supplied by the caller because only they know what their peer does. The suite checks
    /// that it is *answered*, never what the answer says.
    pub call: (&'a str, serde_json::Value),
}

/// Run a peer through the protocol and report everything it got wrong.
///
/// An empty result means it conforms. Findings are independent: the suite keeps going after
/// one so a peer author sees the whole list rather than fixing them one run at a time.
#[must_use]
pub fn check(subject: &Subject<'_>) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut child = match Command::new(subject.command)
        .args(subject.args)
        .current_dir(subject.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return vec![Finding {
                rule: "the peer must be runnable",
                detail: format!("{} could not be started: {e}", subject.command),
            }];
        }
    };

    let Some(stdin) = child.stdin.take() else {
        return vec![Finding {
            rule: "the peer must read its stdin",
            detail: "no stdin pipe".to_owned(),
        }];
    };
    let Some(stdout) = child.stdout.take() else {
        return vec![Finding {
            rule: "the peer must write its stdout",
            detail: "no stdout pipe".to_owned(),
        }];
    };
    let mut writer = FrameWriter::new(stdin);
    let reports = reader_thread(stdout);

    check_declaration(&reports, subject, &mut found);
    check_call(&mut writer, &reports, subject, &mut found);
    check_unknown_tool(&mut writer, &reports, &mut found);
    check_exit(writer, &mut child, &mut found);

    let _ = child.kill();
    let _ = child.wait();
    found
}

/// A peer must say what it offers before it is asked anything.
fn check_declaration(
    reports: &Receiver<Result<ToolReport, String>>,
    subject: &Subject<'_>,
    found: &mut Vec<Finding>,
) {
    let mut declared = Vec::new();
    // The first report only: a declaration must come before anything else, and one is enough
    // to carry on with. A peer offering several declares each, and the rest arrive as they do.
    if let Some(report) = next(reports, PATIENCE) {
        match report {
            Ok(ToolReport::Declare {
                name, parameters, ..
            }) => {
                if name.is_empty() {
                    found.push(Finding {
                        rule: "a declared tool must have a name",
                        detail: "one declaration had an empty name".to_owned(),
                    });
                }
                if parameters.get("type").and_then(|t| t.as_str()) != Some("object") {
                    found.push(Finding {
                        rule: "a declared schema must be a JSON Schema object",
                        detail: format!("{name} declared {parameters}"),
                    });
                }
                declared.push(name);
            }
            Ok(other) => found.push(Finding {
                rule: "a peer must declare before it reports anything else",
                detail: format!("it sent {other:?} first"),
            }),
            Err(e) => found.push(Finding {
                rule: "a peer must stay on the wire",
                detail: e,
            }),
        }
    }

    if declared.is_empty() {
        found.push(Finding {
            rule: "a peer must declare at least one tool on connect",
            detail: format!("nothing was declared within {PATIENCE:?}"),
        });
    } else if !declared.iter().any(|n| n == subject.call.0) {
        found.push(Finding {
            rule: "a peer must declare the tools it answers",
            detail: format!(
                "it declared {declared:?}, which does not include {:?}",
                subject.call.0
            ),
        });
    }
}

/// A call must come back, quoting the id it was given.
fn check_call(
    writer: &mut FrameWriter<std::process::ChildStdin>,
    reports: &Receiver<Result<ToolReport, String>>,
    subject: &Subject<'_>,
    found: &mut Vec<Finding>,
) {
    let id = ToolCallId::new("conformance-1");
    if writer
        .write_blocking(&ToolRequest::Call {
            id: id.clone(),
            name: subject.call.0.to_owned(),
            arguments: subject.call.1.clone(),
        })
        .is_err()
    {
        found.push(Finding {
            rule: "a peer must keep reading its stdin",
            detail: "the call could not be written".to_owned(),
        });
        return;
    }
    match await_result(reports, &id) {
        Some(true) => {}
        Some(false) => found.push(Finding {
            rule: "a result must quote the id of the call it answers",
            detail: "the peer answered with a different id".to_owned(),
        }),
        None => found.push(Finding {
            rule: "every call must be answered",
            detail: format!("{:?} went unanswered within {PATIENCE:?}", subject.call.0),
        }),
    }
}

/// A call for something the peer does not offer is still a call, and still needs an answer.
fn check_unknown_tool(
    writer: &mut FrameWriter<std::process::ChildStdin>,
    reports: &Receiver<Result<ToolReport, String>>,
    found: &mut Vec<Finding>,
) {
    let id = ToolCallId::new("conformance-2");
    if writer
        .write_blocking(&ToolRequest::Call {
            id: id.clone(),
            name: "__no_such_tool__".to_owned(),
            arguments: serde_json::json!({}),
        })
        .is_err()
    {
        return;
    }
    if await_result(reports, &id).is_none() {
        found.push(Finding {
            rule: "a call for an unknown tool must be answered, not ignored",
            detail: "silence leaves the host waiting out its timeout on a mistake it \
                     could have been told about"
                .to_owned(),
        });
    }
}

/// Closing the peer's input must end it.
fn check_exit(
    writer: FrameWriter<std::process::ChildStdin>,
    child: &mut std::process::Child,
    found: &mut Vec<Finding>,
) {
    drop(writer);
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => return,
        }
    }
    found.push(Finding {
        rule: "a peer must exit when its input closes",
        detail: format!("it was still running {PATIENCE:?} after stdin closed"),
    });
}

/// Wait for a `Result`, and say whether it quoted the right id.
fn await_result(reports: &Receiver<Result<ToolReport, String>>, id: &ToolCallId) -> Option<bool> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match next(reports, remaining)? {
            Ok(ToolReport::Result { id: got, .. }) => return Some(got == *id),
            // Progress and late declarations are allowed at any time.
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

fn next(
    reports: &Receiver<Result<ToolReport, String>>,
    within: Duration,
) -> Option<Result<ToolReport, String>> {
    reports.recv_timeout(within).ok()
}

/// Turn the peer's stdout into a channel, so every wait can be given a deadline.
fn reader_thread(stdout: std::process::ChildStdout) -> Receiver<Result<ToolReport, String>> {
    let (reports, incoming) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = FrameReader::new(stdout);
        loop {
            let message = reader
                .read_blocking::<ToolReport>()
                .map_err(|e| e.to_string());
            let failed = message.is_err();
            if reports.send(message).is_err() || failed {
                return;
            }
        }
    });
    incoming
}
