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

/// Ask for presses, releases, drags and where the pointer is.
///
/// `?1002h` reports presses, releases, the wheel, and motion *while a button is held* — a click
/// and a drag. `?1003h` adds motion with no button down, which is the only way to know what the
/// pointer is merely *over*, and is what lights a fold handle up under it. It is a message per
/// cell crossed, so nothing redraws for motion that does not change which handle is lit; the
/// cost of the rest is a parse. `?1006h` asks for SGR coordinates, without which a click past
/// column 223 cannot be expressed.
///
/// **Not `EnableMouseCapture`.** That sets `?1015h` as well — urxvt coordinates, an alternative
/// to SGR that crossterm asks for beside it and takes whichever arrives. One encoding is enough
/// when it is the one everything speaks.
///
/// **Taking the mouse is why magi selects text itself.** Mouse reporting is one terminal-wide
/// switch: an application that turns it on to receive a click stops the terminal running its own
/// drag-selection, and no choice of tracking mode changes that. So a program that wants both a
/// clickable element and selectable text has to do the selecting — see [`magi_tui::select`].
/// neovim and tmux are in the same position and answer it the same way.
const MOUSE_ON: &str = "\x1b[?1002h\x1b[?1003h\x1b[?1006h";

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
                "magi needs a terminal. Run it from a shell, or use `--socket` with a UI\n\
                 that has one. To watch a session without a terminal, read the journal."
            );
        }
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, crossterm::event::EnableBracketedPaste)?;
        execute!(out, EnterAlternateScreen)?;
        // Cleared first, because a killed run leaves its modes set and `?1003h` from an older
        // build is one nothing here would otherwise undo. Then asked for what a click, a drag
        // and the wheel need. See [`MOUSE_ON`] for why magi does the selecting.
        write!(out, "{MOUSE_OFF}{MOUSE_ON}")?;
        out.flush()?;

        let enhanced = push_keyboard_enhancements(&mut out).unwrap_or(false);
        // Not set from `enhanced`: asking for the flags says nothing about whether the terminal
        // honoured them. The first `Repeat` or `Release` to arrive says it, and nothing else does.

        let terminal = Terminal::with_options(
            CrosstermBackend::new(out),
            TerminalOptions {
                viewport: Viewport::Fullscreen,
            },
        )?;
        Ok(Self { terminal, enhanced })
    }
}

/// Whether the terminal this process is attached to reports key holds.
///
/// A process-global for the same reason the session's drain handle is one: there is a single
/// terminal, it is negotiated once before anything is drawn, and it cannot change afterwards. The
/// connection task needs it and runs nowhere near the [`Session`] that learned it.
static REPORTS_HOLDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this process's terminal reports key repeats and releases.
///
/// **Learned rather than predicted.** The flags are asked for unconditionally, so this cannot be
/// answered by whether the asking worked — nothing says whether the terminal honoured it. What
/// does say is a `Repeat` or a `Release` actually arriving: no terminal sends one unless the
/// protocol is live. So this is `false` until the first one is seen and true forever after, which
/// is the difference between believing a probe and knowing.
#[must_use]
pub fn reports_holds() -> bool {
    REPORTS_HOLDS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Note that a key repeat or release arrived, and say whether that is news.
///
/// `true` only the first time, so a caller can tell an open surface once rather than on every
/// keystroke for the rest of the session.
pub fn noticed_hold() -> bool {
    !REPORTS_HOLDS.swap(true, std::sync::atomic::Ordering::Relaxed)
}

/// Ask for the Kitty keyboard protocol, so Shift+Enter is distinguishable from Enter.
///
/// `supports_keyboard_enhancement` probes with the DA1 sentinel Pi uses: a terminal that does
/// not implement the protocol still answers the device-attributes query that follows it, so
/// the absence of a reply is proven by DA1 arriving rather than by a timeout expiring.
///
/// **`REPORT_EVENT_TYPES` is what makes a held key knowable.** Without it a terminal sends one
/// indistinguishable press per repeat and never says when a key came back up, so nothing can tell
/// "tapped" from "still holding" — which is the difference between a hop and a jump in anything
/// that reads the keyboard as a control rather than as text. With it, presses, repeats and
/// releases arrive labelled.
///
/// It changes what the *prompt* sees too: a held key now arrives as `Repeat` where it used to
/// arrive as another `Press`. Whatever reads keys has to accept both, or holding backspace stops
/// deleting on the terminals that support this and keeps working on the ones that do not.
/// **Asked for unconditionally, not gated on a probe.**
///
/// `supports_keyboard_enhancement` is a query and a reply: it writes `CSI ? u` followed by a
/// device-attributes request and reads stdin for the answer. Under a multiplexer that is three
/// things that must all go right — the query forwarded out, the reply forwarded back, and neither
/// racing whatever else is about to read stdin — and when any of them does not, the probe says
/// "unsupported" about a terminal that supports it perfectly well. Gated on that, magi never even
/// asked, so a held key arrived as a stream of presses and a jump re-fired on every landing.
///
/// The push costs nothing to be wrong about. `CSI > flags u` is defined so a terminal that does
/// not implement it ignores it, and so is the pop. Asking and being ignored is strictly better
/// than not asking because a round trip through a mux did not come back.
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

/// Ask for releases on keys that produce text, for as long as something needs them.
///
/// **Not at startup, and this is the whole point.** `REPORT_EVENT_TYPES` does not report a release
/// for a key that generates text — space, a letter — so a surface reading a *hold* needs
/// `REPORT_ALL_KEYS_AS_ESCAPE_CODES` as well. Asked for globally that broke the prompt: with every
/// key arriving as an escape code, `:` stopped opening the command line.
///
/// So it is pushed when a surface takes the keyboard and popped when it gives it back. The flags
/// are a stack in the protocol, which is exactly the shape this wants: the prompt's own layer is
/// still underneath, untouched, and returns the moment the layer above is dropped.
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
