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
    /// What this session calls itself: `project/id`. The left of the footer.
    pub identity: String,
    /// Model id, as the provider names it. The right of the footer.
    pub model: String,
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
    // Held clear at both ends, and the same at both: the prompt box above draws a border in
    // column zero and stops one short of the right, so a footer running edge to edge under it
    // read as leaning left. Everything below measures against the inset width, not the screen.
    let pad = usize::from(crate::metric::footer_pad());
    let width = usize::from(width).saturating_sub(pad * 2);
    let gap = usize::from(crate::metric::column_gap());

    // Ends first, and the shorter of the two has priority: what the session calls itself is
    // fixed for the whole run, and the model is what you check before sending something.
    let name = fit_path(&data.identity, width / 3);
    let model = fit_path(
        &data.model,
        width.saturating_sub(name.chars().count() + gap * 2),
    );
    let name_width = name.chars().count();
    let model_at = width.saturating_sub(model.chars().count());

    // Then the middle, which is the display and whatever it has to say. Centred on the row
    // rather than laid after the name: each column is placed from the width alone, so one of
    // them changing -- and the middle changes every time the agent starts or stops -- does not
    // slide the other two sideways.
    let said: usize = status.iter().map(|s| s.content.chars().count()).sum();
    let middle_at = width.saturating_sub(said) / 2;
    // Centred in the whole row is not the same as fitting between the other two. On a narrow
    // screen the middle reached the right-hand column and the two printed into each other --
    // `12.5%/200kaxum/main/al`. Pushed off centre rather than dropped: the display is the one
    // thing here that says the session is alive.
    let middle_at = middle_at
        .max(name_width + gap)
        .min(model_at.saturating_sub(said + gap));

    let mut spans = vec![Span::styled(" ".repeat(pad), dim)];
    spans.push(Span::styled(name, dim));
    let mut col = name_width;
    if middle_at >= col && middle_at + said + gap <= model_at {
        spans.push(Span::styled(" ".repeat(middle_at - col), dim));
        spans.extend(status.iter().cloned());
        col = middle_at + said;
    }
    if model_at >= col {
        spans.push(Span::styled(" ".repeat(model_at - col), dim));
    }
    spans.push(Span::styled(model, muted));

    let mut row = vec![spans.remove(0)];
    row.extend(clip_spans(spans, width));
    row.push(Span::styled(" ".repeat(pad), dim));
    vec![Line::from(row)]
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
    fn the_three_columns_are_the_name_the_display_and_the_model() {
        // The working directory, the branch and the mouse state had it. Two of them never change
        // while the session runs, and the third has the whole terminal to announce itself. The
        // status took the row above the box, which is a row of chrome for one word.
        let data = FooterData {
            input_tokens: 1200,
            output_tokens: 340,
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            ..FooterData::default()
        };
        let status = [Span::raw("⣠⣾⠀⠀⠀")];
        let rendered = text_of(&render(&data, &status, 60));
        assert!(
            rendered[0].trim_start().starts_with("axum/main/alpha"),
            "the name has the left: {:?}",
            rendered[0]
        );
        assert!(
            rendered[0].contains("⣠⣾⠀⠀⠀"),
            "the display has the middle: {:?}",
            rendered[0]
        );
        assert!(
            rendered[0].trim_end().ends_with("claude-opus-5"),
            "the model has the right: {:?}",
            rendered[0]
        );
        assert!(
            !rendered[0].contains("↑1.2k"),
            "and the numbers are not here any more: {:?}",
            rendered[0]
        );
    }

    #[test]
    fn the_model_is_right_aligned() {
        let data = FooterData {
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, &[], 60));
        assert!(
            rendered[0].trim_end().ends_with("claude-opus-5"),
            "{:?}",
            rendered[0]
        );
        // Still the full width: the model is against the inset edge, and the inset is drawn.
        assert_eq!(rendered[0].chars().count(), 60);
    }

    #[test]
    fn an_unknown_context_percentage_renders_as_a_question_mark() {
        // Worn by the prompt box now rather than the footer, but it is still this that builds it.
        let data = FooterData {
            context_window: 200_000,
            context_percent: None,
            ..FooterData::default()
        };
        assert!(usage(&data).contains("?/200k"), "{:?}", usage(&data));
    }

    #[test]
    fn no_context_window_is_no_context_group() {
        // `?/0` is three characters of noise on exactly the screen a new person is reading.
        assert_eq!(usage(&FooterData::default()), "");
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
    fn the_name_shows_even_with_nothing_else_to_report() {
        let data = FooterData {
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            ..FooterData::default()
        };
        let out = render(&data, &[], 60);
        assert!(
            line_text(&out, 0).contains("axum/main/alpha"),
            "{}",
            line_text(&out, 0)
        );
        assert!(
            line_text(&out, 0).contains("claude-opus-5"),
            "{}",
            line_text(&out, 0)
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
            identity: "a-very-long-project-name/a-very-long-id".into(),
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
            model: "claude-opus-5".into(),
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
    fn what_the_agent_is_doing_does_not_move_the_ends() {
        // The complaint this answers: the two ends slid sideways every time the middle changed,
        // which is every time a turn starts or ends. The display is fixed-width now, which is
        // most of the answer, but a middle that grows must still not push anything.
        let short = row("⣠⣾⠀⠀⠀");
        let long = row(&"⣿".repeat(20));
        for line in [&short, &long] {
            assert_eq!(line.chars().count(), 70, "{line:?}");
        }
        assert_eq!(
            column(&short, "axum/main/alpha"),
            column(&long, "axum/main/alpha"),
            "the name moved:\n{short:?}\n{long:?}"
        );
        assert_eq!(
            column(&short, "claude-opus-5"),
            column(&long, "claude-opus-5"),
            "the model moved:\n{short:?}\n{long:?}"
        );
    }

    #[test]
    fn the_name_is_against_the_left_edge() {
        let line = row("⣠⣾⠀⠀⠀");
        assert!(line.trim_start().starts_with("axum/main/alpha"), "{line:?}");
    }

    #[test]
    fn a_middle_too_long_for_its_room_is_dropped_rather_than_shoving() {
        // The ends are what the row is for. A middle with nowhere to go goes nowhere.
        let line = row(&"x".repeat(200));
        assert_eq!(line.chars().count(), 70, "{line:?}");
        assert!(line.trim_start().starts_with("axum/main/alpha"), "{line:?}");
        assert!(line.trim_end().ends_with("claude-opus-5"), "{line:?}");
    }
}

/// Both ends are held clear, and nothing is allowed to print into anything else.
#[cfg(test)]
mod inset_tests {
    use super::*;

    fn row(width: u16, identity: &str) -> String {
        let data = FooterData {
            input_tokens: 12_500,
            output_tokens: 900,
            context_percent: Some(6.2),
            context_window: 200_000,
            identity: identity.into(),
            model: "claude-opus-5".into(),
        };
        render(&data, &[Span::raw("waiting")], width)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn both_ends_are_held_clear() {
        let pad = usize::from(crate::metric::footer_pad());
        let line = row(80, "axum/main/alpha");
        let head: String = line.chars().take(pad).collect();
        let tail: String = line.chars().skip(line.chars().count() - pad).collect();
        assert!(head.trim().is_empty(), "the left end: {line:?}");
        assert!(tail.trim().is_empty(), "the right end: {line:?}");
        assert_eq!(line.chars().count(), 80, "and the row is still the width");
    }

    #[test]
    fn the_middle_never_prints_into_the_name() {
        // `12.5%/200kaxum/main/al`, which is what a centred middle does once the row is inset
        // and nobody checks it against the column to its right.
        for width in 30..90u16 {
            let line = row(width, "axum/main/alpha");
            assert!(
                !line.contains("kaxum") && !line.contains("%axum"),
                "width {width}: {line:?}"
            );
            assert_eq!(line.chars().count(), usize::from(width), "width {width}");
        }
    }
}

/// The usage, as one string: what went up, what came down, and how full the window is.
///
/// Public because it is no longer drawn here. It had the middle of the footer and the middle is
/// now the display; it is worn by the prompt box instead, in the inverted strip down its right,
/// which is where you are looking when the number matters. Empty when there is nothing to say --
/// `?/0` is three characters of noise on exactly the screen a new person is trying to read.
#[must_use]
pub fn usage(data: &FooterData) -> String {
    let mut parts = Vec::new();
    if data.input_tokens > 0 {
        parts.push(format!("↑{}", format_tokens(data.input_tokens)));
    }
    if data.output_tokens > 0 {
        parts.push(format!("↓{}", format_tokens(data.output_tokens)));
    }
    if data.context_window > 0 {
        parts.push(match data.context_percent {
            Some(pct) => format!("{pct:.1}%/{}", format_tokens(data.context_window)),
            None => format!("?/{}", format_tokens(data.context_window)),
        });
    }
    parts.join(" ")
}

/// The colour the usage is worth: context pressure is the one number here that is ever urgent.
#[must_use]
pub fn pressure(data: &FooterData) -> ratatui::style::Color {
    match data.context_percent {
        Some(p) if p > 90.0 => colour::error(),
        Some(p) if p > 70.0 => colour::warning(),
        _ => colour::hint(),
    }
}

/// And the display lands on the exact middle of the screen, not just of the space it was given.
#[cfg(test)]
mod middle_tests {
    use super::*;

    #[test]
    fn the_display_sits_on_the_screens_own_middle() {
        // The two ends are pinned to the edges, so anything off-centre between them is visible.
        // Checked at both parities of terminal width, which is the case that needed the work.
        for screen in 60..160u16 {
            let cells = crate::beacon::fitted(screen);
            let data = FooterData {
                identity: "axum/main/alpha".into(),
                model: "claude-opus-5".into(),
                ..FooterData::default()
            };
            let marks = vec![Span::raw("#".repeat(cells))];
            let line: String = render(&data, &marks, screen)[0]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            let at = line.find('#').map(|byte| line[..byte].chars().count());
            let Some(at) = at else {
                panic!("width {screen}: the display was dropped from {line:?}");
            };
            // Its own middle against the screen's: equal space either side, to the column.
            let after = usize::from(screen) - at - cells;
            assert_eq!(
                at, after,
                "width {screen}: {at} columns before it and {after} after"
            );
        }
    }
}
