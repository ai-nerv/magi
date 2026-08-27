//! The status line.
//!
//! Pi renders two lines while idle so the layout does not jump when work starts. The spinner
//! is accent-coloured and the message muted, matching `WorkingStatusIndicator`.

use crate::theme::Theme;
use axum_proto::AgentStatus;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Spinner frames, in order. Pi's `Loader` cycles braille at roughly 80ms.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Milliseconds per spinner frame.
pub const FRAME_MS: u64 = 80;

/// Render the status line.
///
/// `tick` advances the spinner; the caller increments it on a timer so rendering stays a pure
/// function of state.
#[must_use]
pub fn render(status: &AgentStatus, tick: usize, theme: &Theme) -> Line<'static> {
    match status {
        AgentStatus::Idle => Line::default(),
        AgentStatus::Working { label } => spinner(label.clone(), tick, theme.accent, theme),
        AgentStatus::Retrying {
            attempt,
            max_attempts,
            delay_ms,
        } => {
            let seconds = delay_ms.div_ceil(1000);
            let label =
                format!("Retrying ({attempt}/{max_attempts}) in {seconds}s... (esc to cancel)");
            spinner(label, tick, theme.warning, theme)
        }
    }
}

fn spinner(
    label: String,
    tick: usize,
    spinner_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    let frame = FRAMES[tick % FRAMES.len()];
    Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(frame.to_owned(), Style::default().fg(spinner_color)),
        Span::styled(" ", Style::default()),
        Span::styled(label, Style::default().fg(theme.muted)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn idle_renders_nothing() {
        let line = render(&AgentStatus::Idle, 0, &Theme::default());
        assert_eq!(text_of(&line), "");
    }

    #[test]
    fn working_shows_a_spinner_and_the_label() {
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        assert_eq!(
            text_of(&render(&status, 0, &Theme::default())),
            " ⠋ Thinking"
        );
    }

    #[test]
    fn the_spinner_advances_with_the_tick() {
        let status = AgentStatus::Working { label: "x".into() };
        let a = text_of(&render(&status, 0, &Theme::default()));
        let b = text_of(&render(&status, 1, &Theme::default()));
        assert_ne!(a, b);
    }

    #[test]
    fn a_retry_rounds_its_delay_up_to_whole_seconds() {
        let status = AgentStatus::Retrying {
            attempt: 2,
            max_attempts: 5,
            delay_ms: 1500,
        };
        assert!(
            text_of(&render(&status, 0, &Theme::default())).contains("(2/5) in 2s"),
            "{}",
            text_of(&render(&status, 0, &Theme::default()))
        );
    }
}
