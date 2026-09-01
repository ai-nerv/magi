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
/// `tick` advances the display; the caller increments it on a timer so rendering stays a pure
/// function of state.
#[must_use]
pub fn render(status: &AgentStatus, tick: usize) -> Line<'static> {
    working(mood_of(status, true), tick)
}

/// The same, saying so when the daemon cannot be reached.
#[must_use]
pub fn connected(status: &AgentStatus, tick: usize, connected: bool) -> Line<'static> {
    working(mood_of(status, connected), tick)
}

/// What the display shows, from the agent alone.
///
/// The UI picks its own — it knows about the prompt having text in it and about a list being
/// open, and neither of those is anything the agent reports. This is the fallback for callers
/// that only have the agent's word for it.
#[must_use]
pub fn mood_of(status: &AgentStatus, connected: bool) -> crate::beacon::Mood {
    if !connected {
        return crate::beacon::Mood::Away;
    }
    match status {
        AgentStatus::Idle => crate::beacon::Mood::Resting,
        AgentStatus::Working { .. } | AgentStatus::Retrying { .. } => crate::beacon::Mood::Working,
    }
}

/// The display, and nothing else.
///
/// Words used to stand beside it here and they have gone to the prompt box, where there is room
/// for them and where you are already looking. What is left is five cells of braille that never
/// change width -- so nothing on the footer row moves when a turn starts or stops, which was the
/// whole complaint about the line this replaced.
#[must_use]
pub fn working(mood: crate::beacon::Mood, tick: usize) -> Line<'static> {
    Line::from(crate::beacon::render(mood, tick))
}

/// What the prompt box says about the turn it is waiting on.
///
/// In the box rather than the footer because the box is where you are looking while you wait,
/// and because it is the one place with room for a sentence. It takes the placeholder's slot --
/// which means it shows while the prompt is empty and gets out of the way the moment you type,
/// and typing during a turn is allowed and always was.
///
/// Empty when there is nothing to say, which is what the placeholder falls back to.
#[must_use]
pub fn effort(status: &AgentStatus, elapsed: Option<std::time::Duration>) -> String {
    match status {
        AgentStatus::Idle => String::new(),
        AgentStatus::Retrying {
            attempt,
            max_attempts,
            delay_ms,
        } => {
            // No elapsed clock: the countdown already says how long, and two numbers that both
            // look like seconds and mean different things is worse than one.
            let seconds = delay_ms.div_ceil(1000);
            format!("retrying ({attempt}/{max_attempts}) in {seconds}s — esc to cancel")
        }
        AgentStatus::Working { label } => {
            let doing = label.to_lowercase();
            // Only once there is something to say. A clock that appears reading `0s` on every
            // turn is noise for the nine turns in ten that finish before anyone looks at it.
            match elapsed.filter(|e| e.as_secs() >= 1) {
                Some(elapsed) => format!(
                    "{doing} for {} — esc to interrupt, or type ahead",
                    format_elapsed(elapsed)
                ),
                None => doing,
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Whether every character of `text` is a braille cell.
    fn all_braille(text: &str) -> bool {
        !text.is_empty() && text.chars().all(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
    }

    #[test]
    fn idle_is_the_display_and_no_words() {
        // "waiting" was here and it is gone. It was read once and never again, and the four
        // cells say the same thing without asking to be read at all.
        let said = text_of(&render(&AgentStatus::Idle, 0));
        assert!(all_braille(&said), "{said:?}");
        assert_eq!(said.chars().count(), crate::beacon::cells());
    }

    #[test]
    fn idle_does_not_spin() {
        // A turning frame beside a word says work is happening, which is the one thing this
        // state means is not. What is there instead never grows a label.
        for tick in 0..32 {
            let said = text_of(&render(&AgentStatus::Idle, tick));
            assert!(all_braille(&said), "tick {tick}: {said:?}");
        }
    }

    #[test]
    fn working_is_the_display_and_no_words_either() {
        // The label went to the prompt box. Nothing on this row changes width when a turn
        // starts, which was the whole complaint about the line this replaced.
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let said = text_of(&render(&status, 0));
        assert!(all_braille(&said), "{said:?}");
        assert_eq!(said.chars().count(), crate::beacon::cells());
    }

    #[test]
    fn the_display_advances_with_the_tick() {
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
        let said = effort(&status, None);
        assert!(said.contains("(2/5) in 2s"), "{said}");
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_lost_daemon_is_shown_not_said() {
        // A UI with no socket looks exactly like an idle one: the prompt takes text and a
        // submitted turn goes into a channel nobody is reading. The word for it is gone with
        // every other word on this row, so the display is what has to carry it.
        let away = text(&connected(&AgentStatus::Idle, 0, false));
        let idle = text(&connected(&AgentStatus::Idle, 0, true));
        assert_ne!(
            away, idle,
            "{away} reads the same as a session that is fine"
        );
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
        assert_eq!(
            text(&line),
            text(&connected(&AgentStatus::Idle, 0, false)),
            "the daemon being away outranks whatever it last said"
        );
    }

    #[test]
    fn a_connected_idle_session_is_not_an_absent_one() {
        // Both are "nothing is happening" and they used to draw the same nothing. The whole
        // reason the display has an `Away` state is that a UI with no daemon behind it looked
        // exactly like an idle one.
        let idle = text(&connected(&AgentStatus::Idle, 0, true));
        let away = text(&connected(&AgentStatus::Idle, 0, false));
        assert_ne!(idle, away);
    }
}

#[cfg(test)]
mod elapsed_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_running_turn_says_how_long_it_has_been_running() {
        // A spinner alone makes ten seconds and thirty look the same, which is how a hung
        // turn passes for a slow one.
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let said = effort(&status, Some(Duration::from_secs(12)));
        assert!(said.contains("12s"), "{said}");
    }

    #[test]
    fn the_first_second_is_not_worth_a_clock() {
        let status = AgentStatus::Working {
            label: "Thinking".into(),
        };
        let said = effort(&status, Some(Duration::from_millis(200)));
        assert!(!said.contains("0s"), "{said}");
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
        let said = effort(&status, Some(Duration::from_secs(3)));
        assert!(said.contains("esc"), "{said}");
    }

    #[test]
    fn a_retry_keeps_its_own_countdown_and_no_second_clock() {
        // Two numbers that both look like seconds and mean different things is worse than one.
        let status = AgentStatus::Retrying {
            attempt: 2,
            max_attempts: 5,
            delay_ms: 4000,
        };
        let text = effort(&status, Some(Duration::from_secs(30)));
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
/// The scroll note in the status line said how much was below, in words, in one place. That is
/// the wrong shape for the question a reader actually has, which is "is this the end" — asked
/// constantly, answered by glancing at the edge rather than by reading a number somewhere else.
/// So the edge itself says it.
///
/// In the box's colour, not the quotation rule's. These two and the prompt box are the only
/// lines axon draws around the whole width, and three edges in two colours reads as two kinds of
/// edge when there is only one kind: here is where something stops.
#[must_use]
pub fn more(width: u16) -> Line<'static> {
    let dash = crate::glyph::more_rule();
    let repeats = usize::from(width) / dash.chars().count().max(1);
    Line::from(Span::styled(
        dash.repeat(repeats),
        Style::default().fg(colour::border()),
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
    fn it_is_drawn_in_the_boxs_colour() {
        // The prompt box and these two rules are the only full-width lines on the screen. In
        // different colours they read as two kinds of edge, and there is only one kind.
        let rule = more(20);
        assert_eq!(rule.spans[0].style.fg, Some(colour::border()));
    }
}
