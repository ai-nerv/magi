//! Terminal setup and teardown, on the alternate screen always.

use anyhow::Result;
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::{self, IsTerminal, Stdout, Write};

/// Presses, drags, motion with no button down, and the SGR coordinates a click past column 223
/// needs. Taking the mouse stops the terminal's own drag-selection: see [`magi_tui::select`].
const MOUSE_ON: &str = "\x1b[?1002h\x1b[?1003h\x1b[?1006h";

/// Given back on the way in as well as out, because a run killed before teardown leaves modes set.
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

/// A terminal in raw mode, restored on drop.
pub struct Session {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    enhanced: bool,
}

impl Session {
    pub fn open() -> Result<Self> {
        // Checked before raw mode: `enable_raw_mode` on a pipe fails with a bare ENXIO.
        if !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "magi needs a terminal. Run it from a shell, or use `--socket` with a UI\n\
                 that has one. To watch a session without a terminal, read the journal."
            );
        }
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, crossterm::event::EnableBracketedPaste)?;
        execute!(out, EnterAlternateScreen)?;
        // Cleared first, because a killed run leaves its modes set and nothing else undoes them.
        write!(out, "{MOUSE_OFF}{MOUSE_ON}")?;
        out.flush()?;

        let enhanced = push_keyboard_enhancements(&mut out).unwrap_or(false);
        // Not set from `enhanced`: only a `Repeat` or `Release` says the terminal honoured it.

        let terminal = Terminal::with_options(
            CrosstermBackend::new(out),
            TerminalOptions {
                viewport: Viewport::Fullscreen,
            },
        )?;
        Ok(Self { terminal, enhanced })
    }
}

/// Process-global: one terminal, and the connection task reads it far from any [`Session`].
static REPORTS_HOLDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this terminal reports key repeats and releases; only a `Repeat` or `Release` arriving
/// says the protocol is live, so this is learned rather than predicted from the push succeeding.
#[must_use]
pub fn reports_holds() -> bool {
    REPORTS_HOLDS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Note a key repeat or release arrived; `true` only the first time, so a surface is told once.
pub fn noticed_hold() -> bool {
    !REPORTS_HOLDS.swap(true, std::sync::atomic::Ordering::Relaxed)
}

/// Ask for the Kitty keyboard protocol, so Shift+Enter differs from Enter and a held key arrives as
/// `Repeat` rather than another `Press` — readers must accept both. Asked for unconditionally: the
/// `supports_keyboard_enhancement` round trip is one a multiplexer breaks, and an unsupported
/// `CSI > flags u` is ignored.
fn push_keyboard_enhancements(out: &mut Stdout) -> Result<bool> {
    queue!(
        out,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        )
    )?;
    out.flush()?;
    Ok(true)
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut out = io::stdout();
        if self.enhanced {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        let _ = write!(out, "{MOUSE_OFF}");
        let _ = execute!(out, crossterm::cursor::SetCursorStyle::DefaultUserShape);
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = execute!(out, crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = self.terminal.show_cursor();
    }
}

/// The cursor shape for a mode: an underline rather than a bar, which wants a column between cells.
#[must_use]
pub fn shape(mode: magi_tui::vim::Mode) -> crossterm::cursor::SetCursorStyle {
    if mode.is_insert() {
        crossterm::cursor::SetCursorStyle::SteadyUnderScore
    } else {
        crossterm::cursor::SetCursorStyle::SteadyBlock
    }
}

/// Ask for releases on keys that produce text, pushed only while a surface holds the keyboard:
/// `REPORT_ALL_KEYS_AS_ESCAPE_CODES` set globally stops `:` opening the command line.
pub fn hold_keys(want: bool) {
    let mut out = io::stdout();
    if want {
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        );
    } else {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = out.flush();
}
