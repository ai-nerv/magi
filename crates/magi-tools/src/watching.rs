//! What a watcher can be told, and the names those things go by. The events here are the ones where
//! a watcher can do something a configuration cannot: time a turn, keep an audit trail of what was
//! permitted, attach state to a session, or see what left the context window. Told after the fact
//! and answered with nothing — see [`Watch`].

/// Something that happened, on its way to every watcher. Borrowed rather than owned: these are
/// emitted on paths that run every turn, and most sessions register no watcher at all.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    /// A tool ran to completion, whether or not it worked.
    Tool {
        name: &'a str,
        /// What it actually ran with, which is not always what was asked for.
        arguments: &'a serde_json::Value,
        is_error: bool,
    },
    /// A turn began — one exchange with the model, before anything is sent.
    TurnBegan {
        /// The model it is going to, as `provider/model`.
        model: &'a str,
    },
    /// A turn finished, successfully or not.
    TurnEnded {
        model: &'a str,
        took_ms: u64,
        ok: bool,
    },
    /// A permission was put to whoever answers them.
    Asked {
        /// `read`, `write`, `run`, `reach`.
        verb: &'a str,
        /// What it was about — a path, a command, a host.
        about: &'a str,
    },
    /// And what came back; separate from [`Event::Asked`] because the gap is a person deciding.
    Answered {
        verb: &'a str,
        about: &'a str,
        allowed: bool,
    },
    /// A session started, or was picked up again.
    Session { id: &'a str, resumed: bool },
    /// The context window was compacted, which leaves nothing in the transcript saying what it took.
    Compacted { dropped: usize, kept: usize },
    /// The mind was asked again after an attempt failed. Said on the status line and then gone.
    Retried {
        /// Which program is doing the asking — melchior, or whatever stands in for it.
        mind: &'a str,
        attempt: u32,
        of: u32,
        delay_ms: u64,
    },
}

impl Event<'_> {
    /// The name a watcher matches on: stable, lowercase and dotted. Written out rather than derived
    /// from the variant name, because renaming one breaks configurations nobody here can see.
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

    /// The event as the value a watcher outside Rust receives. `kind` is in the object rather than
    /// beside it, and a tool event keeps the field names `tool`, `arguments` and `is_error`.
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

/// Something told what happened, after it has happened, and answered with nothing: a watcher that
/// could change a result would be a tool wearing a different name. Several may be registered, each
/// told in turn. Not `Send + Sync`, like [`crate::Tool`]: the interesting watchers live in the Lua VM.
pub trait Watch {
    fn saw(&self, event: &Event<'_>);
}

/// The watchers of one session, shared by everything that has something to report: the registry
/// raises tool events, the turn loop raises turn and compaction events, and the permission gate
/// raises the ones about what was allowed. `Rc` rather than `Arc`, because they live in the Lua VM.
#[derive(Default, Clone)]
pub struct Watchers(std::rc::Rc<std::cell::RefCell<Vec<Box<dyn Watch>>>>);

impl Watchers {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, watcher: Box<dyn Watch>) {
        self.0.borrow_mut().push(watcher);
    }

    #[must_use]
    pub fn any(&self) -> bool {
        self.0.borrow().is_empty().eq(&false)
    }

    /// Tell every watcher, and let none of them matter. A watcher that watches from inside a watcher
    /// is skipped rather than a panic: these are re-entrant by construction.
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
        match self.0.try_borrow() {
            Ok(watching) => write!(f, "Watchers({})", watching.len()),
            Err(_) => f.write_str("Watchers(busy)"),
        }
    }
}

/// A permission question and the answer it got, owned because it outlives the call — see [`Pending`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Noted {
    pub verb: String,
    pub about: String,
    pub allowed: bool,
}

/// Permission questions waiting to be told to somebody. A permission is decided through `Ops`, which
/// is `Send + Sync`, and a [`Watch`] is not — so the gate writes them down and the turn loop reads
/// them out between rounds, on the thread where the watchers are. They arrive slightly late and in
/// order, so a watcher timing the wait has to read `waited_ms` rather than its own clock.
#[derive(Default, Clone, Debug)]
pub struct Pending(std::sync::Arc<std::sync::Mutex<Vec<Noted>>>);

impl Pending {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write one down. A poisoned lock drops it rather than propagating.
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
        // The compatibility promise: `config/tools.lua` reads `event.tool` and `event.arguments`.
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
