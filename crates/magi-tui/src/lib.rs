//! Rendering for the magi UI: [`magi_proto`] types in, styled lines out. Everything here is a pure
//! function of state — no sockets, sessions or agents — which is what makes it testable against a
//! `vt100` screen. Block shapes and footer format are Pi's; the palette is not, see [`colour`].

pub mod beacon;
pub mod border;
pub mod colour;
pub mod complete;
pub mod corner;
pub mod cost;
pub mod decrypt;
pub mod editor;
pub mod fold;
pub mod footer;
pub mod fuzzy;
pub mod glyph;
pub mod markdown;
pub mod menu;
pub mod metric;
pub mod overlay;
pub mod painted;
pub mod pane;
pub mod pick;
pub mod picker;
pub mod prompt;
pub mod scrollback;
pub mod select;
pub mod status;
pub mod table;
pub mod tease;
pub mod trace;
pub mod transcript;
pub mod trigger;
pub mod vim;
pub mod wrap;

pub use editor::Editor;
pub use footer::FooterData;
