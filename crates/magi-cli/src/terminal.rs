//! Terminal setup and teardown.
//!
//! The alternate screen, always. There was an inline backend beside it that kept the terminal's
//! own scrollback, and it is gone: every feature the transcript grew — the wheel, the scroll
//! keys, the edge rule, clicking a block open — was written against a buffer magi owns, and none
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

/// The mouse is the terminal's.
///
/// magi asks for no tracking at all. Mouse reporting is one terminal-wide switch: an application
/// that turns it on to receive a click stops the terminal — and the multiplexer above it — from
/// running their own drag-selection, and no choice of tracking mode changes that. Selecting text
/// and copying it is the thing a terminal is *for*, it already works everywhere, and no click
/// target magi could offer is worth taking it away.
///
/// Sent on the way in as well as the way out, and every mode rather than only the ones magi used
/// to ask for: a run killed before it could tear down leaves whatever it had set still set, and
/// a terminal left in `?1002h` by an older build is one nothing else would clear.
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
                "magi needs a terminal. Run it from a shell, or use `--socket` with a UI\n\
                 that has one. To watch a session without a terminal, read the journal."
            );
        }
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, crossterm::event::EnableBracketedPaste)?;
        execute!(out, EnterAlternateScreen)?;
        // Cleared and left cleared. See [`MOUSE_OFF`]: the mouse belongs to the terminal, and
        // an older build that took it leaves modes set that nothing else here would undo.
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
        // Whatever shape the modes left it in is magi's, not the terminal's, and a shell that
        // came back with a bar cursor would have been given one by us.
        let _ = execute!(out, crossterm::cursor::SetCursorStyle::DefaultUserShape);
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = execute!(out, crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = self.terminal.show_cursor();
    }
}

/// The shape the terminal draws its own cursor in, for a mode.
///
/// A block sits on a character and an underline sits below the one the next letter will push
/// along, which is exactly the difference between the modes: normal mode acts on what is under
/// the cursor, insert mode puts the next character where the cursor is.
///
/// An underline rather than a bar, and the ghost cursor wears the same pair for the same reason:
/// a bar wants a column *between* two cells and a terminal grid has not got one, so the two
/// cursors on this screen would have been drawn by different rules and looked it.
///
/// Steady rather than blinking in both, because the prompt box already has a scan travelling
/// round it and two things pulsing in one corner of the screen is one too many.
#[must_use]
pub fn shape(mode: magi_tui::vim::Mode) -> crossterm::cursor::SetCursorStyle {
    if mode.is_insert() {
        crossterm::cursor::SetCursorStyle::SteadyUnderScore
    } else {
        crossterm::cursor::SetCursorStyle::SteadyBlock
    }
}
