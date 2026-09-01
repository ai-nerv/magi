//! The footer.
//!
//! Two dim lines, as Pi renders them: the working directory with the git branch, then usage
//! stats on the left with the session name right-aligned.

use crate::colour;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// What the footer displays. The UI owns none of this; the daemon reports it.
#[derive(Debug, Clone, Default)]
pub struct FooterData {
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
    /// Percentage of the context window in use.
    pub context_percent: Option<f64>,
    /// Size of the context window, in tokens.
    pub context_window: u64,
    /// What this session calls itself: `project/role/id`.
    ///
    /// The right of the footer, where the model used to be. The two changed places: the model is
    /// what you check mid-turn and belongs against the box you are typing into, and a name that
    /// does not change for the life of the session belongs out at the edge.
    pub identity: String,
}

/// Abbreviate a token count the way Pi's `formatTokens` does.
#[must_use]
pub fn format_tokens(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=9_999 => format!("{:.1}k", count as f64 / 1000.0),
        10_000..=999_999 => format!("{}k", count.div_ceil(1000).saturating_sub(0)),
        1_000_000..=9_999_999 => format!("{:.1}M", count as f64 / 1_000_000.0),
        _ => format!("{}M", count / 1_000_000),
    }
}

/// Collapse a path under the home directory to a `~` prefix.
#[must_use]
pub fn format_cwd(cwd: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|h| !h.is_empty()) else {
        return cwd.to_owned();
    };
    if cwd == home {
        return "~".to_owned();
    }
    cwd.strip_prefix(&format!("{home}/"))
        .map_or_else(|| cwd.to_owned(), |rest| format!("~/{rest}"))
}

/// Fit a path into `width`, dropping leading components rather than trailing ones.
///
/// The old `clip` took the head and cut the tail, which on a long path hides the only part
/// that says where you are: `/home/you/work/deep/nested/thing` became `/home/you/work/dee…`.
/// Leading components are the ones a reader can infer.
#[must_use]
pub fn fit_path(path: &str, width: usize) -> String {
    if path.chars().count() <= width {
        return path.to_owned();
    }
    // No room at all is not a licence to overflow: a caller with nothing left to give gets
    // nothing back. Returning the original here is how the footer once printed a sixty-column
    // model name onto a twenty-column terminal.
    if width == 0 {
        return String::new();
    }
    // Whole components while any fit, so the result is still a path and not a cut word.
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    for skip in 1..parts.len() {
        let tail = format!("…/{}", parts[skip..].join("/"));
        if tail.chars().count() <= width {
            return tail;
        }
    }
    // The last component alone is too long: keep its end, which is the distinctive part.
    let last = parts.last().copied().unwrap_or(path);
    let keep = width.saturating_sub(1);
    let start = last.chars().count().saturating_sub(keep);
    format!("…{}", last.chars().skip(start).collect::<String>())
}

/// Render the footer.
///
/// **One line.** It was two — the directory on its own row above the stats — and two rows of
/// dim text under the prompt is a lot of screen for something you glance at. Everything that
/// was on both is here: the directory and branch on the left, usage in the middle, the session name on
/// the right, and each is dropped in that order when the terminal cannot hold it.
#[must_use]
pub fn render(data: &FooterData, status: &[Span<'static>], width: u16) -> Vec<Line<'static>> {
    let dim = Style::default().fg(colour::dim());
    let muted = Style::default().fg(colour::muted());
    let width = usize::from(width);

    // Right first: the session name is fixed-width for the whole run, so it is the last to go.
    let named = fit_path(
        &data.identity,
        width.saturating_sub(usize::from(crate::metric::column_gap())),
    );
    let room =
        width.saturating_sub(named.chars().count() + usize::from(crate::metric::column_gap()));

    // Then usage, which is short and changes every turn.
    let mut stats = Vec::new();
    if data.input_tokens > 0 {
        stats.push(format!("↑{}", format_tokens(data.input_tokens)));
    }
    if data.output_tokens > 0 {
        stats.push(format!("↓{}", format_tokens(data.output_tokens)));
    }
    // Nothing to say about a context window nobody has: `?/0` is three characters of noise on
    // exactly the screen a new person is trying to read.
    let context = if data.context_window == 0 {
        String::new()
    } else {
        match data.context_percent {
            Some(pct) => format!("{pct:.1}%/{}", format_tokens(data.context_window)),
            None => format!("?/{}", format_tokens(data.context_window)),
        }
    };
    if !context.is_empty() {
        stats.push(context.clone());
    }
    let usage = stats.join(" ");
    let usage = if usage.chars().count() + usize::from(crate::metric::column_gap()) <= room {
        usage
    } else {
        String::new()
    };

    // Context pressure is the one thing in the footer worth breaking the dim palette for.
    let context_color = match data.context_percent {
        Some(p) if p > 90.0 => colour::error(),
        Some(p) if p > 70.0 => colour::warning(),
        _ => colour::dim(),
    };

    // One row under the box, and everything said about the session is on it: what the agent is
    // doing on the left, usage in the middle, the session name on the right. The working directory, the
    // branch and the mouse state had the left of this and are gone -- two of them never change
    // while the session runs, and the third has the whole terminal to announce itself with.
    //
    // Each is placed from the width alone. They were laid out one after another, which meant a
    // change in the left-hand segment -- and it changes every time the agent starts or stops
    // doing something -- slid the other two sideways. Three things that move whenever any one of
    // them moves is a row nobody can read a number off.
    let gap = usize::from(crate::metric::column_gap());
    let name_at = width.saturating_sub(named.chars().count());
    let usage_at = width.saturating_sub(usage.chars().count()) / 2;

    let mut spans: Vec<Span<'static>> = clip_spans(
        status.to_vec(),
        if usage.is_empty() { name_at } else { usage_at }.saturating_sub(gap),
    );
    let mut col: usize = spans.iter().map(|s| s.content.chars().count()).sum();

    if !usage.is_empty() && usage_at >= col {
        spans.push(Span::styled(" ".repeat(usage_at - col), dim));
        let split = usage.len() - context.len();
        if context.is_empty() {
            spans.push(Span::styled(usage.clone(), dim));
        } else {
            spans.push(Span::styled(usage[..split].to_owned(), dim));
            spans.push(Span::styled(context, Style::default().fg(context_color)));
        }
        col = usage_at + usage.chars().count();
    }
    if name_at >= col {
        spans.push(Span::styled(" ".repeat(name_at - col), dim));
    }
    spans.push(Span::styled(named, muted));

    vec![Line::from(clip_spans(spans, width))]
}

/// Trim a styled line to `width`, dropping whole spans and then characters.
///
/// The last guard on the stats line. Every part of it is fitted on its own, but a terminal
/// narrow enough that the token counts alone overflow leaves nothing to fit -- and a line that
/// overflows wraps, which costs the footer a row it was not given.
fn clip_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0usize;
    for span in spans {
        let len = span.content.chars().count();
        if used + len <= width {
            used += len;
            out.push(span);
            continue;
        }
        let room = width.saturating_sub(used);
        if room > 0 {
            let kept: String = span.content.chars().take(room).collect();
            out.push(Span::styled(kept, span.style));
        }
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn token_counts_abbreviate_like_pi() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn home_collapses_to_a_tilde() {
        assert_eq!(format_cwd("/home/me", Some("/home/me")), "~");
        assert_eq!(format_cwd("/home/me/src", Some("/home/me")), "~/src");
        assert_eq!(format_cwd("/etc", Some("/home/me")), "/etc");
    }

    #[test]
    fn what_the_agent_is_doing_has_the_left() {
        // The working directory, the branch and the mouse state had it. Two of them never change
        // while the session runs, and the third has the whole terminal to announce itself. The
        // status took the row above the box, which is a row of chrome for one word.
        let data = FooterData {
            input_tokens: 1200,
            output_tokens: 340,
            identity: "m".into(),
            ..FooterData::default()
        };
        let status = [Span::raw("⠋ thinking")];
        let rendered = text_of(&render(&data, &status, 60));
        assert!(rendered[0].starts_with("⠋ thinking"), "{:?}", rendered[0]);
        assert!(rendered[0].contains("↑1.2k ↓340"), "{:?}", rendered[0]);
        assert!(rendered[0].ends_with('m'), "{:?}", rendered[0]);
    }

    #[test]
    fn the_session_name_is_right_aligned() {
        let data = FooterData {
            context_window: 200_000,
            context_percent: Some(12.5),
            identity: "axum/main/alpha".into(),
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, &[], 40));
        assert!(
            rendered[0].ends_with("axum/main/alpha"),
            "{:?}",
            rendered[0]
        );
        assert_eq!(rendered[0].chars().count(), 40);
    }

    #[test]
    fn an_unknown_context_percentage_renders_as_a_question_mark() {
        let data = FooterData {
            context_window: 200_000,
            context_percent: None,
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, &[], 40));
        assert!(rendered[0].contains("?/200k"), "{:?}", rendered[0]);
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    fn line_text(lines: &[Line<'_>], row: usize) -> String {
        lines[row]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_long_path_keeps_the_end_that_says_where_you_are() {
        // The old clip took the head: `/home/you/work/deep/nested/thing` became
        // `/home/you/work/dee…`, which names every directory except the one you are in.
        let fitted = fit_path("/home/you/work/deep/nested/thing", 20);
        assert!(fitted.ends_with("thing"), "{fitted}");
        assert!(fitted.chars().count() <= 20, "{fitted}");
    }

    #[test]
    fn a_path_that_fits_is_left_alone() {
        assert_eq!(fit_path("~/work", 40), "~/work");
    }

    #[test]
    fn whole_components_survive_rather_than_half_a_word() {
        let fitted = fit_path("/aaa/bbb/ccc/ddd", 12);
        assert!(fitted.starts_with("…/"), "{fitted}");
        assert!(!fitted.contains("…/bb"), "no half components: {fitted}");
    }

    #[test]
    fn one_enormous_component_keeps_its_tail() {
        let fitted = fit_path("/x/abcdefghijklmnop", 8);
        assert!(fitted.ends_with("mnop"), "{fitted}");
        assert!(fitted.chars().count() <= 8, "{fitted}");
    }

    #[test]
    fn no_context_window_is_no_context_group() {
        // `?/0` is three characters of noise on exactly the screen a new person is reading.
        let data = FooterData {
            identity: "axum/main/alpha".into(),
            ..FooterData::default()
        };
        let out = render(&data, &[], 40);
        assert!(!line_text(&out, 0).contains("?/"), "{}", line_text(&out, 0));
        assert!(
            line_text(&out, 0).contains("axum/main/alpha"),
            "the session name still shows"
        );
    }
}

#[cfg(test)]
mod name_fit_tests {
    use super::*;

    fn stats_row(data: &FooterData, width: u16) -> String {
        render(data, &[], width)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_long_name_keeps_the_part_that_names_it() {
        // Right-aligned text is cut on the left by the terminal and on the right by us; either
        // way `a-long-project/main/alpha` must not become `a-long-project/main`.
        let data = FooterData {
            identity: "a-long-project/main/alpha".into(),
            context_window: 164_000,
            context_percent: Some(0.0),
            ..FooterData::default()
        };
        let row = stats_row(&data, 30);
        assert!(row.contains("alpha"), "{row}");
        assert!(row.chars().count() <= 30, "{row}");
    }

    #[test]
    fn a_name_that_fits_is_left_alone() {
        let data = FooterData {
            identity: "axum/main/beta".into(),
            ..FooterData::default()
        };
        assert!(stats_row(&data, 80).contains("axum/main/beta"));
    }

    #[test]
    fn the_line_never_outgrows_the_terminal() {
        let data = FooterData {
            identity: "a-very-long-project-name/a-very-long-role/a-very-long-id".into(),
            input_tokens: 123_456,
            output_tokens: 654_321,
            context_window: 200_000,
            ..FooterData::default()
        };
        for width in [20u16, 30, 40, 60, 100] {
            let row = stats_row(&data, width);
            assert!(
                row.chars().count() <= usize::from(width),
                "width {width}: {row}"
            );
        }
    }
}

/// Each segment is placed from the width, so one changing does not move the others.
#[cfg(test)]
mod anchored {
    use super::*;

    fn data() -> FooterData {
        FooterData {
            input_tokens: 12_500,
            output_tokens: 900,
            context_percent: Some(6.2),
            context_window: 200_000,
            identity: "axum/main/alpha".into(),
        }
    }

    /// Which column `needle` starts at, counted in characters rather than bytes.
    fn column(row: &str, needle: &str) -> Option<usize> {
        row.find(needle).map(|byte| row[..byte].chars().count())
    }

    /// The row rendered with `said` on the left.
    fn row(said: &str) -> String {
        let status = [Span::raw(said.to_owned())];
        render(&data(), &status, 70)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn what_the_agent_is_doing_does_not_move_the_numbers() {
        // The complaint this answers: the usage and the right-hand column slid sideways every time the
        // status changed, which is every time a turn starts or ends.
        let short = row("waiting");
        let long = row("⠋ Thinking  12s  esc to interrupt");
        for row in [&short, &long] {
            assert_eq!(row.chars().count(), 70, "{row:?}");
        }
        assert_eq!(
            column(&short, "↑13k"),
            column(&long, "↑13k"),
            "usage moved:\n{short:?}\n{long:?}"
        );
        assert_eq!(
            column(&short, "openrouter"),
            column(&long, "openrouter"),
            "the session name moved:\n{short:?}\n{long:?}"
        );
    }

    #[test]
    fn the_session_name_is_against_the_right_edge() {
        assert!(row("waiting").ends_with("axum/main/alpha"));
    }

    #[test]
    fn a_status_too_long_for_its_room_is_cut_rather_than_shoving() {
        // It gets what is left of the row before the numbers, and no more.
        let row = row(&"x".repeat(200));
        assert_eq!(row.chars().count(), 70, "{row:?}");
        assert!(row.contains("↑13k"), "the numbers survived: {row:?}");
        assert!(row.ends_with("axum/main/alpha"), "{row:?}");
    }
}
