//! Terminal setup and teardown.
//!
//! Two backends, one renderer. Everything that draws lives in `axum-tui` and produces
//! `Vec<Line>`; the only thing a backend decides is where settled transcript lines go.
//!
//! Pi ships both of these too, and it is listed as a weakness in the architecture brief — but
//! the fault there is not that two exist, it is that they behave differently and the user
//! cannot tell which one they are in. Both of ours draw from the same component code and
//! offer the same capabilities, and the footer names the active one.

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

/// Which backend draws the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// A live region at the bottom of the normal screen; settled lines are handed to the
    /// terminal, which keeps the history. Native scroll, search, and copy keep working.
    Inline,
    /// The alternate screen, where axum owns every cell and the whole transcript.
    ///
    /// The terminal has no history here, so we keep it: which is the price of admission for
    /// transcript search, selection, and jump-to-message, none of which can reach into a
    /// terminal's own scrollback. The default, for that reason.
    #[default]
    Alt,
}

impl Mode {
    /// The name shown in the footer, so the active backend is never a guess.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Alt => "alt",
        }
    }
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "inline" => Ok(Self::Inline),
            "alt" | "altscreen" | "alt-screen" => Ok(Self::Alt),
            other => Err(format!("unknown tui mode {other:?}; use `inline` or `alt`")),
        }
    }
}

/// A terminal in raw mode, restored on drop.
pub struct Session {
    /// The ratatui terminal, kept live for the duration of the UI.
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Which backend this session is drawing with.
    pub mode: Mode,
    /// Whether the keyboard protocol was pushed and must be popped.
    enhanced: bool,
}

impl Session {
    /// Enter raw mode and claim the screen.
    ///
    /// `height` sizes the live region and is ignored in [`Mode::Alt`], which takes the lot.
    pub fn open(mode: Mode, height: u16) -> Result<Self> {
        // Checked before raw mode rather than after: `enable_raw_mode` on a pipe fails with a
        // bare ENXIO, which tells a user piping the UI nothing about what they did wrong.
        if !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "axum needs a terminal. Run it from a shell, or use `--socket` with a UI\n\
                 that has one. To watch a session without a terminal, read the journal."
            );
        }
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, crossterm::event::EnableBracketedPaste)?;
        if mode == Mode::Alt {
            execute!(out, EnterAlternateScreen)?;
            // Only here. The alternate screen replaces the terminal's own scrollback, so the
            // wheel has nothing to move unless this program moves it — without this the
            // transcript simply could not be scrolled. Inline mode keeps the terminal's
            // scrollback, so capturing there would take the wheel away from something that
            // already works, and take selection and copy with it.
            execute!(out, crossterm::event::EnableMouseCapture)?;
        }

        let enhanced = push_keyboard_enhancements(&mut out).unwrap_or(false);

        let viewport = match mode {
            Mode::Inline => Viewport::Inline(height),
            Mode::Alt => Viewport::Fullscreen,
        };
        let terminal =
            Terminal::with_options(CrosstermBackend::new(out), TerminalOptions { viewport })?;
        Ok(Self {
            terminal,
            mode,
            enhanced,
        })
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
        if self.mode == Mode::Alt {
            let _ = execute!(out, crossterm::event::DisableMouseCapture);
            let _ = execute!(out, LeaveAlternateScreen);
        }
        let _ = execute!(out, crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = self.terminal.show_cursor();
        if self.mode == Mode::Inline {
            let _ = writeln!(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_from_their_names() {
        assert_eq!("inline".parse::<Mode>(), Ok(Mode::Inline));
        assert_eq!("alt".parse::<Mode>(), Ok(Mode::Alt));
        assert_eq!("alt-screen".parse::<Mode>(), Ok(Mode::Alt));
        assert!("fullscreen".parse::<Mode>().is_err());
    }

    #[test]
    fn the_default_owns_the_buffer() {
        assert_eq!(
            Mode::default(),
            Mode::Alt,
            "a buffer we own is the only one a later feature can search or select in"
        );
    }

    #[test]
    fn every_mode_names_itself_for_the_footer() {
        assert_eq!(Mode::Inline.label(), "inline");
        assert_eq!(Mode::Alt.label(), "alt");
    }
}

/// The wheel, and why capture is per-mode.
#[cfg(test)]
mod wheel {
    use super::Mode;

    #[test]
    fn only_the_alternate_screen_takes_the_mouse() {
        // Alt mode replaces the terminal's own scrollback, so nothing moves the transcript
        // unless this program moves it — that was the whole of "i cant scroll". Inline mode
        // keeps the terminal's scrollback, and capturing there would take away a wheel that
        // already works, along with drag-selection and copy.
        assert_eq!(Mode::default(), Mode::Alt, "the mode that needs the wheel");
        assert_ne!(Mode::Inline, Mode::Alt);
    }
}
