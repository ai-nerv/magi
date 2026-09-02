//! Putting text on the clipboard, through the terminal.
//!
//! OSC 52, because it is the only clipboard that works everywhere magi runs. A local one — X11,
//! Wayland — is the wrong shape twice over: magi is often on the far side of ssh, where the
//! clipboard that matters is the one in front of the person, and it is often inside a
//! multiplexer, which forwards OSC 52 and knows nothing about anybody's display server.
//!
//! Written to the terminal rather than to stdout's usual writer because it is a control string,
//! not output: it must not go through the frame ratatui is composing.

use base64::Engine as _;
use std::io::Write;

/// The most that is sent.
///
/// A terminal parses an OSC string into one buffer, and a multiplexer forwarding it often has a
/// smaller one than the terminal behind it. Past this a copy is dropped rather than sent as a
/// truncation nobody asked for: half a file on the clipboard is worse than none, because it
/// looks like it worked.
const MOST: usize = 64 * 1024;

/// Put `text` on the system clipboard.
pub fn put(text: &str) {
    if text.len() > MOST {
        return;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    // `c` is the selection: the clipboard proper rather than the primary selection, because
    // what a person expects after dragging is that ctrl+v pastes it.
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_enormous_selection_is_dropped_rather_than_truncated() {
        // Half a file on the clipboard is worse than none: it looks like it worked. Nothing is
        // written, so the terminal's clipboard keeps whatever was on it.
        let huge = "x".repeat(MOST + 1);
        put(&huge);
    }

    #[test]
    fn the_payload_is_base64_of_the_text() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("hello");
        assert_eq!(encoded, "aGVsbG8=");
    }
}
