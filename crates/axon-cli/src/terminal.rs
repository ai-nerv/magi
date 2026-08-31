//! Terminal setup and teardown.
//!
//! The alternate screen, always. There was an inline backend beside it that kept the terminal's
//! own scrollback, and it is gone: every feature the transcript grew — the wheel, the scroll
//! keys, the edge rule, clicking a block open — was written against a buffer axon owns, and none
//! of them worked in the other mode. Two renderers where one is untested is not a choice, it is
//! a second one that quietly does less.

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

/// Ask the terminal to report button presses and the wheel, and nothing else.
///
/// **Not `EnableMouseCapture`.** That sets `?1003h` — *any-event* tracking, which reports every
/// motion of the pointer whether a button is down or not. A terminal in that mode hands the
/// application the whole mouse, and dragging out a selection stops working; several emulators
/// stop honouring shift-to-bypass as well, which leaves no way to select text at all.
///
/// axon reads three events — wheel up, wheel down, and a left press on a block's handle — and
/// `?1000h` reports all three. Drag and motion are what it does not need and what selection does,
/// so they stay with the terminal. `?1006h` asks for SGR coordinates, without which a click past
/// column 223 cannot be expressed.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";

/// Give the mouse back.
///
/// Every mode, not only the two [`MOUSE_ON`] asks for. A run that was killed before it could tear
/// down leaves whatever it had set still set, and a terminal left in `?1003h` by an older build
/// reports motion to a program that never asked for it and will not select text. Sent on the way
/// in as well as the way out, so a session starts from a known state whatever the last one did.
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

/// A terminal in raw mode, restored on drop.
pub struct Session {
    /// The ratatui terminal, kept live for the duration of the UI.
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Whether the keyboard protocol was pushed and must be popped.
    enhanced: bool,
}

impl Session {
    /// Enter raw mode and claim the screen.
    pub fn open() -> Result<Self> {
        // Checked before raw mode rather than after: `enable_raw_mode` on a pipe fails with a
        // bare ENXIO, which tells a user piping the UI nothing about what they did wrong.
        if !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "axon needs a terminal. Run it from a shell, or use `--socket` with a UI\n\
                 that has one. To watch a session without a terminal, read the journal."
            );
        }
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, crossterm::event::EnableBracketedPaste)?;
        execute!(out, EnterAlternateScreen)?;
        // Released rather than taken, and released explicitly: a terminal a killed run left in
        // a tracking mode is one nothing here set and nothing here would clear. See [`MOUSE_OFF`].
        write!(out, "{MOUSE_OFF}")?;
        out.flush()?;

        let enhanced = push_keyboard_enhancements(&mut out).unwrap_or(false);

        let terminal = Terminal::with_options(
            CrosstermBackend::new(out),
            TerminalOptions {
                viewport: Viewport::Fullscreen,
            },
        )?;
        Ok(Self { terminal, enhanced })
    }
}

/// Ask for the Kitty keyboard protocol, so Shift+Enter is distinguishable from Enter.
///
/// `supports_keyboard_enhancement` probes with the DA1 sentinel Pi uses: a terminal that does
/// not implement the protocol still answers the device-attributes query that follows it, so
/// the absence of a reply is proven by DA1 arriving rather than by a timeout expiring.
fn push_keyboard_enhancements(out: &mut Stdout) -> Result<bool> {
    if !crossterm::terminal::supports_keyboard_enhancement()? {
        return Ok(false);
    }
    queue!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
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
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = execute!(out, crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = self.terminal.show_cursor();
    }
}

impl Session {
    /// Take the mouse from the terminal, or give it back.
    ///
    /// Held by default so the wheel scrolls and a block opens under the pointer. What is asked
    /// for is buttons and the wheel, not motion, so the terminal keeps drag-selection either
    /// way — see [`MOUSE_ON`] — but a captured mouse defeats selection in some emulators however
    /// little it asks for, so nothing is captured until this says so.
    pub fn set_mouse(&mut self, holding: bool) {
        let mut out = io::stdout();
        let _ = write!(out, "{}", if holding { MOUSE_ON } else { MOUSE_OFF });
        let _ = out.flush();
    }
}
