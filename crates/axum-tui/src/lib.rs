//! Rendering for the axum UI.
//!
//! Consumes [`axum_proto`] types and produces styled lines. This crate knows nothing about
//! sockets, sessions, or agents: everything here is a pure function of state, which is what
//! makes it testable against a `vt100` screen rather than a live daemon.
//!
//! The visual design is Pi's — palette, block shapes, and footer format all match
//! `xtra/pi/packages/coding-agent/src/modes/interactive`.

pub mod complete;
pub mod editor;
pub mod footer;
pub mod fuzzy;
pub mod markdown;
pub(crate) use complete::fit;

pub mod picker;
pub mod prompt;
pub mod scrollback;
pub mod status;
pub mod theme;
pub mod transcript;
pub mod wrap;

pub use editor::Editor;
pub use footer::FooterData;
pub use theme::{DARK, Theme};
