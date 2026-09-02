//! One axon talking to another.
//!
//! Naming, finding, reaching and refusing: everything about a session's relationship to the
//! other sessions on the machine, and nothing about what a session *is*. There are no turns
//! here, no transcript, no model and no screen.
//!
//! # This crate is on its way out of the workspace
//!
//! It is meant to become its own project, the way `aeon` is the memory layer beside axon rather
//! than a part of it. So the rule is not a preference:
//!
//! > **Nothing in this crate depends on anything named `axon-`.**
//!
//! Not `axon-proto`, not `axon-tools`, not `axon-ipc`. A dependency on any of them would make
//! the eventual move a rewrite instead of a `git mv`, and it would be discovered on the day
//! there was least appetite for it. It costs a `bind` of its own and a `PeerCred` of its own —
//! twenty lines each, in [`serving`] — against a coupling that would have to be undone.
//!
//! The one thing that has to travel back the other way is what the model may call, and that
//! goes as **data**: [`verbs::described`] hands over name, description and JSON schema, and a
//! host turns them into whatever a tool looks like on its side. That is the same arrangement
//! aeon has with axon — the sibling publishes descriptors, the harness registers what comes
//! back — and it is why the vocabulary is written once, here, rather than twice.
//!
//! # The wire is the family's
//!
//! Four-byte big-endian length, then a JSON body. `{"call":…,"args":[…]}` in,
//! `{"ok":true,"n":N,"result":[…]}` out, where **`result` is a list** and `n` is its length.
//! A refusal is a reply, not a dropped connection. A connection serves more than one call.
//! `verbs` is answered from the first version and before any permission check.
//!
//! Not axon's own UI protocol, which is CBOR inside a versioned envelope and right for two
//! halves of one program that ship together. Anything may knock on this socket, so it speaks
//! what the family agreed — see [`framing`], which exists because it once did not.

pub mod answering;
pub mod asking;
pub mod briefing;
pub mod directory;
pub mod framing;
pub mod identity;
pub mod policy;
pub mod serving;
pub mod verbs;
pub mod wire;

pub use directory::{Address, Reach};
pub use identity::Identity;
pub use policy::{Relation, Talk, Whom};
