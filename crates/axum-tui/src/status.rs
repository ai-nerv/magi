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
    connected(status, tick, theme, true)
}

/// The same, saying so when the daemon cannot be reached.
///
/// A UI that has lost its socket looks exactly like an idle one: the prompt accepts text, the
/// transcript sits there, and a submitted turn goes into a channel nobody is reading. The
/// session is not lost — the daemon owns it and the UI redials — but a person typing into
/// silence deserves to be told which silence it is.
#[must_use]
pub fn connected(
    status: &AgentStatus,
    tick: usize,
    theme: &Theme,
    connected: bool,
) -> Line<'static> {
    if !connected {
        return spinner(
            "Reconnecting to the daemon...".to_owned(),
            tick,
            theme.warning,
            theme,
        );
    }
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

#[cfg(test)]
mod connection_tests {
    use super::*;

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_lost_daemon_is_said_out_loud() {
        // A UI with no socket looks exactly like an idle one: the prompt takes text and a
        // submitted turn goes into a channel nobody is reading.
        let line = connected(&AgentStatus::Idle, 0, &Theme::default(), false);
        assert!(text(&line).contains("Reconnecting"), "{}", text(&line));
    }

    #[test]
    fn it_outranks_whatever_the_session_last_said_it_was_doing() {
        // The last status to arrive was "Thinking", and it has not been true since the socket
        // went. Showing it would be reporting a turn that nothing is running.
        let working = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let line = connected(&working, 0, &Theme::default(), false);
        assert!(!text(&line).contains("Thinking"), "{}", text(&line));
    }

    #[test]
    fn a_connected_idle_session_still_says_nothing() {
        // Two lines while idle so the layout does not jump; the words are the exception.
        let line = connected(&AgentStatus::Idle, 0, &Theme::default(), true);
        assert_eq!(text(&line), "");
    }
}
