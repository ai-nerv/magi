//! What a watcher can be told, and the names those things go by.
//!
//! A hook surface with one event is a callback. magi had exactly one — a tool finished — fired
//! from one place, with one shipped consumer, so the only extension anybody could write was one
//! that counted tool calls. The events here are the ones where a watcher can do something a
//! configuration cannot: time a turn, keep an audit trail of what was permitted, attach state to
//! a session, or see what left the context window, which is otherwise entirely silent.
//!
//! **Every one of these already existed as a typed value inside the host.** None of it is new
//! information; what was missing was a name for it and a way out.
//!
//! **Told after the fact, and answered with nothing.** A watcher cannot change what happened and
//! cannot fail it — see [`Watch`] for why that is the whole point rather than a limitation.

/// Something that happened, on its way to every watcher.
///
/// Borrowed rather than owned: these are emitted on paths that run every turn, and a watcher that
/// wants to keep a value can clone the part it wants. The lifetime is what stops this becoming an
/// allocation per event for the sessions — the overwhelming majority — that register none.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    /// A tool ran to completion, whether or not it worked.
    Tool {
        /// Which tool.
        name: &'a str,
        /// What it actually ran with, which is not always what was asked for.
        arguments: &'a serde_json::Value,
        /// Whether it reported a problem.
        is_error: bool,
    },
    /// A turn began — one exchange with the model, before anything is sent.
    TurnBegan {
        /// The model it is going to, as `provider/model`.
        model: &'a str,
    },
    /// A turn finished, successfully or not.
    TurnEnded {
        /// The model it went to.
        model: &'a str,
        /// How long it took, in milliseconds.
        took_ms: u64,
        /// Whether it produced an answer.
        ok: bool,
    },
    /// A permission was put to whoever answers them.
    Asked {
        /// `read`, `write`, `run`, `reach`.
        verb: &'a str,
        /// What it was about — a path, a command, a host.
        about: &'a str,
    },
    /// And what came back.
    ///
    /// Separate from [`Event::Asked`] because the gap between them is a person deciding, and the
    /// interesting thing to record is often how long that took.
    Answered {
        /// `read`, `write`, `run`, `reach`.
        verb: &'a str,
        /// What it was about.
        about: &'a str,
        /// Whether it was allowed.
        allowed: bool,
    },
    /// A session started, or was picked up again.
    Session {
        /// Which session.
        id: &'a str,
        /// Whether this was a resume rather than a fresh start.
        resumed: bool,
    },
    /// The context window was compacted.
    ///
    /// The one event with no other way to observe it: compaction happens between turns and
    /// leaves nothing in the transcript saying what it took out.
    Compacted {
        /// How many entries went.
        dropped: usize,
        /// How many stayed.
        kept: usize,
    },
    /// The mind was asked again after an attempt failed.
    ///
    /// **The thing you want to see when a turn is slow and you do not know why.** It is said on
    /// the status line while it is happening and then it is gone; nothing writes it down.
    Retried {
        /// Which program is doing the asking — melchior, or whatever stands in for it.
        mind: &'a str,
        /// Which attempt just failed, counting from one.
        attempt: u32,
        /// How many will be made in all.
        of: u32,
        /// How long before the next one.
        delay_ms: u64,
    },
}

impl Event<'_> {
    /// The name a watcher matches on.
    ///
    /// Stable, lowercase, and dotted. A watcher branches on this, so renaming one breaks
    /// configurations that nobody here can see — which is why they are written out rather than
    /// derived from the variant name.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Tool { .. } => "tool.finished",
            Self::TurnBegan { .. } => "turn.began",
            Self::TurnEnded { .. } => "turn.ended",
            Self::Asked { .. } => "permission.asked",
            Self::Answered { .. } => "permission.answered",
            Self::Session { .. } => "session.opened",
            Self::Compacted { .. } => "context.compacted",
            Self::Retried { .. } => "provider.retried",
        }
    }

    /// The event as the value a watcher outside Rust receives.
    ///
    /// `kind` is in the object rather than beside it, so a watcher that stores one keeps what it
    /// needs to tell them apart later. The fields of a tool event keep the names they have always
    /// had — `tool`, `arguments`, `is_error` — because configurations were written against them
    /// before any of the others existed.
    #[must_use]
    pub fn value(&self) -> serde_json::Value {
        let mut body = match *self {
            Self::Tool {
                name,
                arguments,
                is_error,
            } => serde_json::json!({ "tool": name, "arguments": arguments, "is_error": is_error }),
            Self::TurnBegan { model } => serde_json::json!({ "model": model }),
            Self::TurnEnded { model, took_ms, ok } => {
                serde_json::json!({ "model": model, "took_ms": took_ms, "ok": ok })
            }
            Self::Asked { verb, about } => serde_json::json!({ "verb": verb, "about": about }),
            Self::Answered {
                verb,
                about,
                allowed,
            } => serde_json::json!({ "verb": verb, "about": about, "allowed": allowed }),
            Self::Session { id, resumed } => serde_json::json!({ "id": id, "resumed": resumed }),
            Self::Compacted { dropped, kept } => {
                serde_json::json!({ "dropped": dropped, "kept": kept })
            }
            Self::Retried {
                mind,
                attempt,
                of,
                delay_ms,
            } => {
                serde_json::json!({ "mind": mind, "attempt": attempt, "of": of, "delay_ms": delay_ms })
            }
        };
        if let Some(fields) = body.as_object_mut() {
            fields.insert("kind".to_owned(), serde_json::json!(self.kind()));
        }
        body
    }
}

/// Something told what happened, after it has happened.
///
/// Told *after* the fact and answered with nothing: a watcher that could change a result would
/// be a tool wearing a different name, and one that could fail would be a way for observation to
/// break the thing observed. Several may be registered; each is told in turn, and none can affect
/// another or the session.
///
/// Not `Send + Sync`, for the same reason [`crate::Tool`] is not: the interesting watchers live
/// in the same VM the Lua tools do, and demanding the bounds would force an `unsafe impl`
/// asserting what the single-threaded design already guarantees.
pub trait Watch {
    /// Something happened.
    fn saw(&self, event: &Event<'_>);
}

/// The watchers of one session, shared by everything that has something to report.
///
/// **A handle rather than a list, because the events do not come from one place.** The registry
/// raises tool events, the turn loop raises turn and compaction events, and the permission gate
/// raises the ones about what was allowed — three owners, one audience. Passing the registry into
/// the gate would be a tool registry deciding permissions; passing the gate into the registry
/// would be the reverse. This is the thing both of them hold.
///
/// `Rc` rather than `Arc`, for the same reason [`Watch`] is not `Send`: the watchers live in the
/// Lua VM, on the thread that owns it.
#[derive(Default, Clone)]
pub struct Watchers(std::rc::Rc<std::cell::RefCell<Vec<Box<dyn Watch>>>>);

impl Watchers {
    /// Nobody watching.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one.
    pub fn add(&self, watcher: Box<dyn Watch>) {
        self.0.borrow_mut().push(watcher);
    }

    /// Whether anything is listening.
    #[must_use]
    pub fn any(&self) -> bool {
        self.0.borrow().is_empty().eq(&false)
    }

    /// Tell every watcher, and let none of them matter.
    ///
    /// **A watcher that watches from inside a watcher is skipped, not a panic.** These are
    /// re-entrant by construction — a Lua watcher runs in the VM, and what it does there can
    /// reach a tool, which raises an event of its own. Refusing the inner report costs that one
    /// observation; taking the borrow anyway would end the session over an observation.
    pub fn saw(&self, event: &Event<'_>) {
        let Ok(watching) = self.0.try_borrow() else {
            return;
        };
        for watcher in watching.iter() {
            watcher.saw(event);
        }
    }
}

impl std::fmt::Debug for Watchers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The watchers themselves have nothing to print — they are closures in another VM — so
        // this says how many there are, which is the only fact about them from out here.
        match self.0.try_borrow() {
            Ok(watching) => write!(f, "Watchers({})", watching.len()),
            Err(_) => f.write_str("Watchers(busy)"),
        }
    }
}

/// A permission question and the answer it got.
///
/// Owned, because it outlives the call that raised it — see [`Pending`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Noted {
    /// `read`, `write`, `run`, `reach`.
    pub verb: String,
    /// What it was about — a path, a command, a host.
    pub about: String,
    /// Whether it was allowed.
    pub allowed: bool,
}

/// Permission questions waiting to be told to somebody.
///
/// **The one place an event cannot be delivered where it happens.** A permission is decided
/// through `Ops`, which is `Send + Sync` because tools run wherever they run; a [`Watch`] is
/// neither, because the interesting watchers live in the Lua VM on the thread that owns it.
/// Handing the watchers to the gate would mean either an `unsafe impl` asserting a bound the
/// design does not have, or moving the VM, and neither is worth an observation.
///
/// So the gate writes them down and the turn loop reads them out between rounds, on the thread
/// where the watchers are. They arrive slightly late and in order, which is what a watcher of an
/// after-the-fact event was promised anyway; nothing here is a hook that could have changed the
/// answer, and a watcher that timed the wait would have to read `waited_ms` rather than its own
/// clock — which is why the wait is not recorded and the two events are reported adjacent.
#[derive(Default, Clone, Debug)]
pub struct Pending(std::sync::Arc<std::sync::Mutex<Vec<Noted>>>);

impl Pending {
    /// Nothing waiting.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write one down.
    ///
    /// A poisoned lock drops it rather than propagating: a permission was still decided
    /// correctly, and losing the record of it is not worth failing the call that raised it.
    pub fn note(&self, noted: Noted) {
        if let Ok(mut waiting) = self.0.lock() {
            waiting.push(noted);
        }
    }

    /// Take everything written down since the last time.
    #[must_use]
    pub fn drain(&self) -> Vec<Noted> {
        self.0
            .lock()
            .map(|mut waiting| std::mem::take(&mut *waiting))
            .unwrap_or_default()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn every_event_carries_its_own_name() {
        // A watcher branches on `kind`, so an event whose value does not carry it is one that
        // cannot be told apart from any other after it has been stored.
        let nothing = serde_json::Value::Null;
        let events = [
            Event::Tool {
                name: "bash",
                arguments: &nothing,
                is_error: false,
            },
            Event::TurnBegan { model: "m" },
            Event::TurnEnded {
                model: "m",
                took_ms: 1,
                ok: true,
            },
            Event::Asked {
                verb: "run",
                about: "git",
            },
            Event::Answered {
                verb: "run",
                about: "git",
                allowed: true,
            },
            Event::Session {
                id: "s",
                resumed: false,
            },
            Event::Compacted {
                dropped: 1,
                kept: 2,
            },
            Event::Retried {
                mind: "melchior",
                attempt: 1,
                of: 3,
                delay_ms: 500,
            },
        ];
        let mut names = std::collections::BTreeSet::new();
        for event in events {
            let value = event.value();
            assert_eq!(value["kind"], serde_json::json!(event.kind()), "{value}");
            assert!(names.insert(event.kind()), "two events share a name");
        }
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn a_tool_event_keeps_the_field_names_configurations_were_written_against() {
        // The one compatibility promise here: `config/tools.lua` reads `event.tool`,
        // `event.arguments` and `event.is_error`, and so does everybody else's.
        let arguments = serde_json::json!({ "command": "ls" });
        let value = Event::Tool {
            name: "bash",
            arguments: &arguments,
            is_error: true,
        }
        .value();
        assert_eq!(value["tool"], serde_json::json!("bash"));
        assert_eq!(value["arguments"]["command"], serde_json::json!("ls"));
        assert_eq!(value["is_error"], serde_json::json!(true));
    }

    #[test]
    fn a_watcher_is_told_and_cannot_answer() {
        // The shape of the contract, pinned: `saw` returns nothing, so there is no value a
        // watcher could return that anything would read.
        struct Counting(RefCell<Vec<String>>);
        impl Watch for Counting {
            fn saw(&self, event: &Event<'_>) {
                self.0.borrow_mut().push(event.kind().to_owned());
            }
        }
        let seen = Counting(RefCell::new(Vec::new()));
        seen.saw(&Event::TurnBegan { model: "m" });
        seen.saw(&Event::Compacted {
            dropped: 3,
            kept: 4,
        });
        assert_eq!(
            seen.0.borrow().as_slice(),
            ["turn.began", "context.compacted"]
        );
    }
}
