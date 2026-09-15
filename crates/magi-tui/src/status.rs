//! The status line: two lines while idle so the layout does not jump when work starts. Accent
//! spinner, muted message.

use crate::colour;
use magi_proto::AgentStatus;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// The spinner, and nothing else. Fixed width, so nothing on the footer row moves when a turn starts.
#[must_use]
pub fn working(
    trace: &mut crate::beacon::Trace,
    mood: crate::beacon::Mood,
    tick: usize,
    screen: u16,
) -> Line<'static> {
    Line::from(crate::beacon::render(
        trace,
        mood,
        tick,
        crate::beacon::fitted(screen),
    ))
}

/// What the prompt box says about the turn it is waiting on. It takes the placeholder's slot, so it
/// shows while the prompt is empty and gets out of the way the moment you type.
#[must_use]
pub fn effort(status: &AgentStatus, elapsed: Option<std::time::Duration>) -> String {
    match status {
        AgentStatus::Idle => String::new(),
        AgentStatus::Retrying {
            attempt,
            max_attempts,
            delay_ms,
        } => {
            // No elapsed clock: the countdown already says how long.
            let seconds = delay_ms.div_ceil(1000);
            format!("retrying ({attempt}/{max_attempts}) in {seconds}s — esc to cancel")
        }
        AgentStatus::Working { label } => {
            let doing = label.to_lowercase();
            // Only once there is something to say; a clock reading `0s` on every turn is noise.
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
/// The note shown while the session is away and work is waiting for it. A prompt submitted while
/// disconnected sits in the command channel and goes out on reconnect rather than being lost.
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
    use crate::beacon::{Mood, Trace};

    /// The row after `frames` frames with `mood` on the wire.
    fn said(mood: Mood, frames: usize) -> String {
        let mut trace = Trace::default();
        let mut out = String::new();
        for tick in 0..frames.max(1) {
            out = working(&mut trace, mood, tick, 80)
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
        }
        out
    }

    /// Whether every character of `text` is a braille cell.
    fn all_braille(text: &str) -> bool {
        !text.is_empty() && text.chars().all(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
    }

    #[test]
    fn the_row_is_the_display_and_no_words() {
        // Nothing on this row changes width when a turn starts.
        for mood in [Mood::Resting, Mood::Working, Mood::Asking, Mood::Away] {
            let out = said(mood, 40);
            assert!(all_braille(&out), "{mood:?}: {out:?}");
            assert_eq!(out.chars().count(), crate::beacon::fitted(80));
        }
    }

    #[test]
    fn a_running_turn_does_not_look_like_an_idle_one() {
        assert_ne!(said(Mood::Working, 40), said(Mood::Resting, 40));
    }

    #[test]
    fn a_lost_session_does_not_look_like_an_idle_session() {
        // A UI with no socket looks exactly like an idle one, so the gaps travelling through the
        // spinner are the whole of what tells them apart.
        assert_ne!(said(Mood::Away, 60), said(Mood::Resting, 60));
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
mod elapsed_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_running_turn_says_how_long_it_has_been_running() {
        // A spinner alone makes ten seconds and thirty look the same.
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
        // An emptied box with nothing on screen gave no way to tell a queued message from a lost one.
        assert!(text(&queued(1)).contains("1 message waiting"));
    }

    #[test]
    fn several_are_plural() {
        assert!(text(&queued(3)).contains("3 messages waiting"));
    }
}

/// A dashed rule across an edge the transcript continues past. In the box's colour, not the
/// quotation rule's: three full-width edges in two colours read as two kinds of edge.
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
        // The prompt box and these two rules are the only full-width lines, and one kind of edge.
        let rule = more(20);
        assert_eq!(rule.spans[0].style.fg, Some(colour::border()));
    }
}
