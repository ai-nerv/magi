//! Rendering for the axum UI.
//!
//! Consumes [`axum_proto`] types and produces styled lines. This crate knows nothing about
//! sockets, sessions, or agents: everything here is a pure function of state, which is what
//! makes it testable against a `vt100` screen rather than a live daemon.
//!
//! The block shapes and footer format are Pi's, from
//! `xtra/pi/packages/coding-agent/src/modes/interactive`. The palette is not, and is not anybody
//! else's either: see [`colour`].

pub mod border;
pub mod colour;
pub mod complete;
pub mod editor;
pub mod footer;
pub mod fuzzy;
pub mod glyph;
pub mod greeting;
pub mod markdown;
pub mod menu;
pub mod metric;

pub mod picker;
pub mod prompt;
pub mod scrollback;
pub mod status;
pub mod table;
pub mod transcript;
pub mod wrap;

pub use editor::Editor;
pub use footer::FooterData;
