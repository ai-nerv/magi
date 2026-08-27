//! Colors, ported from Pi's `dark.json`.
//!
//! The hex values are Pi's, verbatim, so a transcript rendered here reads as the same program.
//! Names follow Pi's `colors` keys rather than being renamed to Rust taste — matching the
//! source makes divergence visible.

use ratatui::style::Color;

/// Pi's `dark` palette.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Primary accent; spinners, list cursors, markdown bullets.
    pub accent: Color,
    /// Success states and added diff lines.
    pub success: Color,
    /// Errors and removed diff lines.
    pub error: Color,
    /// Warnings and elevated context usage.
    pub warning: Color,
    /// Secondary text.
    pub muted: Color,
    /// Tertiary text; the footer lives here.
    pub dim: Color,
    /// The rule above and below the prompt.
    pub border_muted: Color,
    /// Default foreground.
    pub text: Color,
    /// Reasoning blocks, rendered italic.
    pub thinking_text: Color,
    /// Background behind a user message.
    pub user_message_bg: Color,
    /// Foreground of a user message.
    pub user_message_text: Color,
    /// Background behind a tool block that has not finished.
    pub tool_pending_bg: Color,
    /// Background behind a tool block that succeeded.
    pub tool_success_bg: Color,
    /// Background behind a tool block that failed.
    pub tool_error_bg: Color,
    /// Tool name in a tool block header.
    pub tool_title: Color,
    /// Tool output body.
    pub tool_output: Color,
    /// Markdown headings.
    pub md_heading: Color,
    /// Inline code spans.
    pub md_code: Color,
    /// Fenced code block contents.
    pub md_code_block: Color,
    /// Block quote text and its rule.
    pub md_quote: Color,
    /// Added lines in a diff.
    pub diff_added: Color,
    /// Removed lines in a diff.
    pub diff_removed: Color,
    /// Unchanged context lines in a diff.
    pub diff_context: Color,
}

/// Pi's `dark` theme values.
pub const DARK: Theme = Theme {
    accent: Color::Rgb(0x8a, 0xbe, 0xb7),
    success: Color::Rgb(0xb5, 0xbd, 0x68),
    error: Color::Rgb(0xcc, 0x66, 0x66),
    warning: Color::Rgb(0xff, 0xff, 0x00),
    muted: Color::Rgb(0x80, 0x80, 0x80),
    dim: Color::Rgb(0x66, 0x66, 0x66),
    border_muted: Color::Rgb(0x50, 0x50, 0x50),
    text: Color::Rgb(0xd4, 0xd4, 0xd4),
    thinking_text: Color::Rgb(0x80, 0x80, 0x80),
    user_message_bg: Color::Rgb(0x34, 0x35, 0x41),
    user_message_text: Color::Rgb(0xd4, 0xd4, 0xd4),
    tool_pending_bg: Color::Rgb(0x28, 0x28, 0x32),
    tool_success_bg: Color::Rgb(0x28, 0x32, 0x28),
    tool_error_bg: Color::Rgb(0x3c, 0x28, 0x28),
    tool_title: Color::Rgb(0xd4, 0xd4, 0xd4),
    tool_output: Color::Rgb(0x80, 0x80, 0x80),
    md_heading: Color::Rgb(0xf0, 0xc6, 0x74),
    md_code: Color::Rgb(0x8a, 0xbe, 0xb7),
    md_code_block: Color::Rgb(0xb5, 0xbd, 0x68),
    md_quote: Color::Rgb(0x80, 0x80, 0x80),
    diff_added: Color::Rgb(0xb5, 0xbd, 0x68),
    diff_removed: Color::Rgb(0xcc, 0x66, 0x66),
    diff_context: Color::Rgb(0x80, 0x80, 0x80),
};

impl Default for Theme {
    fn default() -> Self {
        DARK
    }
}
