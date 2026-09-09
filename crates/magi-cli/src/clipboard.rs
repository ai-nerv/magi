//! Putting text on the clipboard with OSC 52, written to the terminal rather than stdout so the
//! control string does not enter the frame ratatui is composing.

use base64::Engine as _;
use std::io::Write;

/// Cap on an OSC 52 payload; past this a copy is dropped rather than silently truncated.
const MOST: usize = 64 * 1024;

/// Put `text` on the system clipboard.
pub fn put(text: &str) {
    if text.len() > MOST {
        return;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_enormous_selection_is_dropped_rather_than_truncated() {
        let huge = "x".repeat(MOST + 1);
        put(&huge);
    }

    #[test]
    fn the_payload_is_base64_of_the_text() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("hello");
        assert_eq!(encoded, "aGVsbG8=");
    }
}
