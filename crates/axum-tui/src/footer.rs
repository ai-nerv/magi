//! The footer.
//!
//! Two dim lines, as Pi renders them: the working directory with the git branch, then usage
//! stats on the left with the model right-aligned.

use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// What the footer shows when no model has been chosen.
///
/// Named because three places compare against it, and a string literal repeated three times
/// is a rename waiting to go wrong.
pub const NO_MODEL: &str = "no-model";

/// Minimum gap between the stats and the right-aligned model name.
const MIN_GAP: usize = 2;

/// What the footer displays. The UI owns none of this; the daemon reports it.
#[derive(Debug, Clone, Default)]
pub struct FooterData {
    /// Working directory, already collapsed to `~` where applicable.
    pub cwd: String,
    /// Current git branch, if the working directory is a repository.
    pub branch: Option<String>,
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
    /// Percentage of the context window in use.
    pub context_percent: Option<f64>,
    /// Size of the context window, in tokens.
    pub context_window: u64,
    /// Model id, as the provider names it.
    pub model: String,
    /// Which backend is drawing, shown so the two are never confused for each other.
    pub mode: &'static str,
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
#[must_use]
pub fn render(data: &FooterData, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.dim);

    // The branch and the mode are short and are what changes; the path is long and is what
    // gets cut. Build the suffix first so the path is fitted to what is actually left.
    let mut suffix = String::new();
    if let Some(branch) = &data.branch {
        suffix.push_str(&format!(" ({branch})"));
    }
    if !data.mode.is_empty() {
        suffix.push_str(&format!(" · {}", data.mode));
    }
    let room = usize::from(width).saturating_sub(suffix.chars().count());
    let location = format!("{}{suffix}", fit_path(&data.cwd, room));
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
    // Context pressure is the one thing in the footer worth breaking the dim palette for.
    let context_color = match data.context_percent {
        Some(p) if p > 90.0 => theme.error,
        Some(p) if p > 70.0 => theme.warning,
        _ => theme.dim,
    };

    let left = stats.join(" ");
    let left_width = left.chars().count();
    let context_width = context.chars().count();

    let head = match (left.is_empty(), context.is_empty()) {
        (true, true) => 0,
        (true, false) => context_width,
        (false, true) => left_width,
        (false, false) => left_width + 1 + context_width,
    };
    // The model is right-aligned, so the terminal cuts its tail -- which on `provider/family/
    // model` is the only part that says which model. Fitted from the left instead, the way the
    // path above is: the provider prefix is the half a reader can infer.
    let model = fit_path(
        &data.model,
        usize::from(width).saturating_sub(head + MIN_GAP),
    );
    let right_width = model.chars().count();
    let gap = usize::from(width)
        .saturating_sub(head + right_width)
        .max(MIN_GAP);

    let mut spans = Vec::new();
    if !left.is_empty() {
        spans.push(Span::styled(left.clone(), dim));
        if !context.is_empty() {
            spans.push(Span::styled(" ".to_owned(), dim));
        }
    }
    if !context.is_empty() {
        spans.push(Span::styled(context, Style::default().fg(context_color)));
    }
    spans.push(Span::styled(" ".repeat(gap), dim));
    spans.push(Span::styled(model, dim));

    vec![
        Line::from(Span::styled(clip(&location, usize::from(width)), dim)),
        Line::from(clip_spans(spans, usize::from(width))),
    ]
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

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    text.chars()
        .take(width.saturating_sub(3))
        .collect::<String>()
        + "..."
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
    fn the_active_backend_is_named_beside_the_directory() {
        let data = FooterData {
            cwd: "~".into(),
            mode: "alt",
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, 60, &Theme::default()));
        assert!(rendered[0].contains("alt"), "{:?}", rendered[0]);
    }

    #[test]
    fn the_branch_follows_the_directory() {
        let data = FooterData {
            cwd: "~/src/axum".into(),
            branch: Some("develop".into()),
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, 60, &Theme::default()));
        assert_eq!(rendered[0], "~/src/axum (develop)");
    }

    #[test]
    fn the_model_is_right_aligned() {
        let data = FooterData {
            cwd: "~".into(),
            context_window: 200_000,
            context_percent: Some(12.5),
            model: "claude-opus-5".into(),
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, 40, &Theme::default()));
        assert!(rendered[1].ends_with("claude-opus-5"), "{:?}", rendered[1]);
        assert_eq!(rendered[1].chars().count(), 40);
    }

    #[test]
    fn an_unknown_context_percentage_renders_as_a_question_mark() {
        let data = FooterData {
            context_window: 200_000,
            context_percent: None,
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, 40, &Theme::default()));
        assert!(rendered[1].contains("?/200k"), "{:?}", rendered[1]);
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
    fn the_branch_survives_a_path_too_long_to_fit() {
        // Fitting the whole line cut the branch off with it, and the branch is the half that
        // changes.
        let data = FooterData {
            cwd: "/home/you/a/very/long/path/that/will/not/fit/at/all".into(),
            branch: Some("develop".into()),
            model: "m".into(),
            ..FooterData::default()
        };
        let out = render(&data, 40, &crate::theme::DARK);
        assert!(
            line_text(&out, 0).contains("(develop)"),
            "{}",
            line_text(&out, 0)
        );
    }

    #[test]
    fn no_context_window_is_no_context_group() {
        // `?/0` is three characters of noise on exactly the screen a new person is reading.
        let data = FooterData {
            model: NO_MODEL.into(),
            ..FooterData::default()
        };
        let out = render(&data, 40, &crate::theme::DARK);
        assert!(!line_text(&out, 1).contains("?/"), "{}", line_text(&out, 1));
        assert!(
            line_text(&out, 1).contains(NO_MODEL),
            "the model still shows"
        );
    }
}

#[cfg(test)]
mod model_fit_tests {
    use super::*;

    fn stats_row(data: &FooterData, width: u16) -> String {
        render(data, width, &crate::theme::DARK)[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_long_model_keeps_the_part_that_names_it() {
        // Right-aligned text is cut on the left by the terminal and on the right by us; either
        // way `openrouter/deepseek/deepseek-v3.2` must not become `openrouter/deepseek`.
        let data = FooterData {
            model: "openrouter/deepseek/deepseek-v3.2".into(),
            context_window: 164_000,
            context_percent: Some(0.0),
            ..FooterData::default()
        };
        let row = stats_row(&data, 30);
        assert!(row.contains("deepseek-v3.2"), "{row}");
        assert!(row.chars().count() <= 30, "{row}");
    }

    #[test]
    fn a_model_that_fits_is_left_alone() {
        let data = FooterData {
            model: "ollama/llama3.3".into(),
            ..FooterData::default()
        };
        assert!(stats_row(&data, 80).contains("ollama/llama3.3"));
    }

    #[test]
    fn the_line_never_outgrows_the_terminal() {
        let data = FooterData {
            model: "a-very-long-provider/a-very-long-family/a-very-long-model-name".into(),
            input_tokens: 123_456,
            output_tokens: 654_321,
            context_window: 200_000,
            context_percent: Some(88.8),
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
