//! The status line.
//!
//! Pi renders two lines while idle so the layout does not jump when work starts. The spinner
//! is accent-coloured and the message muted, matching `WorkingStatusIndicator`.

use crate::colour;
use axon_proto::AgentStatus;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Render the status line.
///
/// `tick` advances the spinner; the caller increments it on a timer so rendering stays a pure
/// function of state.
#[must_use]
pub fn render(status: &AgentStatus, tick: usize) -> Line<'static> {
    working(status, tick, true, None)
}

/// The same, saying so when the daemon cannot be reached.
#[must_use]
pub fn connected(status: &AgentStatus, tick: usize, connected: bool) -> Line<'static> {
    working(status, tick, connected, None)
}

/// The status line, with how long the turn has been running.
///
/// A spinner alone says something is happening and nothing about whether to keep waiting. Ten
/// seconds and thirty seconds look identical, which is how a hung turn passes for a slow one.
///
/// A UI that has lost its socket looks exactly like an idle one: the prompt accepts text, the
/// transcript sits there, and a submitted turn goes into a channel nobody is reading. The
/// session is not lost -- the daemon owns it and the UI redials -- but a person typing into
/// silence deserves to be told which silence it is.
#[must_use]
pub fn working(
    status: &AgentStatus,
    tick: usize,
    connected: bool,
    elapsed: Option<std::time::Duration>,
) -> Line<'static> {
    if !connected {
        return spinner(
            "Reconnecting to the daemon...".to_owned(),
            tick,
            colour::warning(),
            None,
        );
    }
    match status {
        AgentStatus::Idle => Line::default(),
        AgentStatus::Working { label } => spinner(label.clone(), tick, colour::accent(), elapsed),
        AgentStatus::Retrying {
            attempt,
            max_attempts,
            delay_ms,
        } => {
            let seconds = delay_ms.div_ceil(1000);
            let label =
                format!("Retrying ({attempt}/{max_attempts}) in {seconds}s... (esc to cancel)");
            // No elapsed clock: the countdown already says how long, and two numbers that
            // both look like seconds and mean different things is worse than one.
            spinner(label, tick, colour::warning(), None)
        }
    }
}

/// The note shown while the daemon is away and work is waiting for it.
///
/// A prompt submitted while disconnected is not lost -- it sits in the command channel and
/// goes out on reconnect -- but the box emptied and nothing appeared, so there was no way to
/// tell a queued message from a swallowed one.
#[must_use]
pub fn queued(count: usize) -> Vec<Span<'static>> {
    if count == 0 {
        return Vec::new();
    }
    let what = if count == 1 { "message" } else { "messages" };
    vec![Span::styled(
        format!("  {count} {what} waiting to send"),
        Style::default().fg(colour::dim()),
    )]
}

/// How long something has been running, at the precision a person reads at.
#[must_use]
pub fn format_elapsed(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        return format!("{seconds}s");
    }
    format!("{}m{:02}s", seconds / 60, seconds % 60)
}

fn spinner(
    label: String,
    tick: usize,
    spinner_color: ratatui::style::Color,
    elapsed: Option<std::time::Duration>,
) -> Line<'static> {
    let frame = crate::glyph::spinner(tick);
    let mut spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled(frame.to_owned(), Style::default().fg(spinner_color)),
        Span::styled(" ", Style::default()),
        Span::styled(label, Style::default().fg(colour::muted())),
    ];
    // Only once there is something to say. A clock that appears reading `0s` on every turn is
    // noise for the nine turns in ten that finish before anyone looks at it.
    if let Some(elapsed) = elapsed.filter(|e| e.as_secs() >= 1) {
        spans.push(Span::styled(
            format!("  {}", format_elapsed(elapsed)),
            Style::default().fg(colour::dim()),
        ));
        spans.push(Span::styled(
            "  esc to interrupt",
            Style::default().fg(colour::dim()),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn idle_renders_nothing() {
        let line = render(&AgentStatus::Idle, 0);
        assert_eq!(text_of(&line), "");
    }

    #[test]
    fn working_shows_a_spinner_and_the_label() {
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        assert_eq!(text_of(&render(&status, 0)), " ⠋ Thinking");
    }

    #[test]
    fn the_spinner_advances_with_the_tick() {
        let status = AgentStatus::Working { label: "x".into() };
        let a = text_of(&render(&status, 0));
        let b = text_of(&render(&status, 1));
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
            text_of(&render(&status, 0)).contains("(2/5) in 2s"),
            "{}",
            text_of(&render(&status, 0))
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
        let line = connected(&AgentStatus::Idle, 0, false);
        assert!(text(&line).contains("Reconnecting"), "{}", text(&line));
    }

    #[test]
    fn it_outranks_whatever_the_session_last_said_it_was_doing() {
        // The last status to arrive was "Thinking", and it has not been true since the socket
        // went. Showing it would be reporting a turn that nothing is running.
        let working = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let line = connected(&working, 0, false);
        assert!(!text(&line).contains("Thinking"), "{}", text(&line));
    }

    #[test]
    fn a_connected_idle_session_still_says_nothing() {
        // Two lines while idle so the layout does not jump; the words are the exception.
        let line = connected(&AgentStatus::Idle, 0, true);
        assert_eq!(text(&line), "");
    }
}

#[cfg(test)]
mod elapsed_tests {
    use super::*;
    use std::time::Duration;

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_running_turn_says_how_long_it_has_been_running() {
        // A spinner alone makes ten seconds and thirty look the same, which is how a hung
        // turn passes for a slow one.
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let line = working(&status, 0, true, Some(Duration::from_secs(12)));
        assert!(text_of(&line).contains("12s"), "{}", text_of(&line));
    }

    #[test]
    fn the_first_second_is_not_worth_a_clock() {
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let line = working(&status, 0, true, Some(Duration::from_millis(200)));
        assert!(!text_of(&line).contains("0s"), "{}", text_of(&line));
    }

    #[test]
    fn a_long_turn_reads_in_minutes() {
        assert_eq!(format_elapsed(Duration::from_secs(90)), "1m30s");
        assert_eq!(format_elapsed(Duration::from_secs(59)), "59s");
        assert_eq!(format_elapsed(Duration::from_secs(3605)), "60m05s");
    }

    #[test]
    fn a_waiting_turn_is_told_how_to_stop_it() {
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let line = working(&status, 0, true, Some(Duration::from_secs(3)));
        assert!(text_of(&line).contains("esc"), "{}", text_of(&line));
    }

    #[test]
    fn a_retry_keeps_its_own_countdown_and_no_second_clock() {
        // Two numbers that both look like seconds and mean different things is worse than one.
        let status = AgentStatus::Retrying {
            attempt: 2,
            max_attempts: 5,
            delay_ms: 4000,
        };
        let line = working(&status, 0, true, Some(Duration::from_secs(30)));
        let text = text_of(&line);
        assert!(text.contains("in 4s"), "{text}");
        assert!(!text.contains("30s"), "{text}");
    }
}

#[cfg(test)]
mod queued_tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn nothing_waiting_is_nothing_said() {
        assert!(queued(0).is_empty());
    }

    #[test]
    fn one_waiting_message_is_singular() {
        // An emptied prompt box with nothing on screen gave no way to tell a queued message
        // from a swallowed one.
        assert!(text(&queued(1)).contains("1 message waiting"));
    }

    #[test]
    fn several_are_plural() {
        assert!(text(&queued(3)).contains("3 messages waiting"));
    }
}

/// A dashed rule across an edge the transcript continues past.
///
/// The scroll note in the status line says how much is below, in words, in one place. That is
/// the wrong shape for the question a reader actually has, which is "is this the end" — asked
/// constantly, answered by glancing at the edge rather than by reading a number somewhere else.
/// So the edge itself says it.
#[must_use]
pub fn more(width: u16) -> Line<'static> {
    let dash = crate::glyph::more_rule();
    let repeats = usize::from(width) / dash.chars().count().max(1);
    Line::from(Span::styled(
        dash.repeat(repeats),
        Style::default().fg(colour::rule()),
    ))
}

#[cfg(test)]
mod more_tests {
    use super::*;

    fn width_of(line: &Line<'_>) -> usize {
        line.spans.iter().map(|s| s.content.chars().count()).sum()
    }

    #[test]
    fn the_rule_fits_the_width_it_is_given() {
        for width in [10u16, 40, 81, 120] {
            let rule = more(width);
            assert!(
                width_of(&rule) <= usize::from(width),
                "{} overflows {width}",
                width_of(&rule)
            );
        }
    }

    #[test]
    fn a_screen_too_narrow_for_one_dash_draws_nothing_rather_than_panicking() {
        assert_eq!(width_of(&more(0)), 0);
    }

    #[test]
    fn it_is_drawn_in_the_rule_colour_so_it_never_competes_with_the_text() {
        let rule = more(20);
        assert_eq!(rule.spans[0].style.fg, Some(colour::rule()));
    }
}
