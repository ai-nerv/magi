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

/// Ask for presses, releases and drags — a click and a drag, nothing else.
///
/// **Not `EnableMouseCapture`.** That sets `?1003h` — *any-event* tracking, which reports the
/// pointer moving with no button down, every frame, forever. `?1002h` reports presses, releases
/// and motion *while a button is held*, which is a click and a drag and nothing else. `?1006h`
/// asks for SGR coordinates, without which a click past column 223 cannot be expressed.
///
/// **Taking the mouse is why axon selects text itself.** Mouse reporting is one terminal-wide
/// switch: an application that turns it on to receive a click stops the terminal running its own
/// drag-selection, and no choice of tracking mode changes that. So a program that wants both a
/// clickable element and selectable text has to do the selecting — see [`axon_tui::select`].
/// neovim and tmux are in the same position and answer it the same way.
const MOUSE_ON: &str = "\x1b[?1002h\x1b[?1006h";

/// Give it back.
///
/// Every mode, not only the two [`MOUSE_ON`] asks for, and sent on the way in as well as the way
/// out: a run killed before it could tear down leaves whatever it had set still set, and a
/// terminal left in `?1003h` by an older build is one nothing here would otherwise clear.
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
        // Cleared first, because a killed run leaves its modes set and `?1003h` from an older
        // build is one nothing here would otherwise undo. Then asked for what a click, a drag
        // and the wheel need. See [`MOUSE_ON`] for why axon does the selecting.
        write!(out, "{MOUSE_OFF}{MOUSE_ON}")?;
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
        // Whatever shape the modes left it in is axon's, not the terminal's, and a shell that
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
/// A block sits on a character and a bar sits between two, which is exactly the difference
/// between the modes: normal mode acts on what is under the cursor, insert mode puts the next
/// character where the cursor is. Steady rather than blinking in both, because the prompt box
/// already has a scan travelling round it and two things pulsing in one corner of the screen is
/// one too many.
#[must_use]
pub fn shape(mode: axon_tui::vim::Mode) -> crossterm::cursor::SetCursorStyle {
    if mode.is_insert() {
        crossterm::cursor::SetCursorStyle::SteadyBar
    } else {
        crossterm::cursor::SetCursorStyle::SteadyBlock
    }
}
