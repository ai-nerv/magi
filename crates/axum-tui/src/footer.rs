//! The footer.
//!
//! Two dim lines, as Pi renders them: the working directory with the git branch, then usage
//! stats on the left with the model right-aligned.

use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

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

/// Render the footer.
#[must_use]
pub fn render(data: &FooterData, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.dim);

    let mut location = data.cwd.clone();
    if let Some(branch) = &data.branch {
        location.push_str(&format!(" ({branch})"));
    }
    if !data.mode.is_empty() {
        location.push_str(&format!(" · {}", data.mode));
    }

    let mut stats = Vec::new();
    if data.input_tokens > 0 {
        stats.push(format!("↑{}", format_tokens(data.input_tokens)));
    }
    if data.output_tokens > 0 {
        stats.push(format!("↓{}", format_tokens(data.output_tokens)));
    }

    let context = match data.context_percent {
        Some(pct) => format!("{pct:.1}%/{}", format_tokens(data.context_window)),
        None => format!("?/{}", format_tokens(data.context_window)),
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
    let right_width = data.model.chars().count();

    let head = if left.is_empty() {
        context_width
    } else {
        left_width + 1 + context_width
    };
    let gap = usize::from(width)
        .saturating_sub(head + right_width)
        .max(MIN_GAP);

    let mut spans = Vec::new();
    if !left.is_empty() {
        spans.push(Span::styled(format!("{left} "), dim));
    }
    spans.push(Span::styled(context, Style::default().fg(context_color)));
    spans.push(Span::styled(" ".repeat(gap), dim));
    spans.push(Span::styled(data.model.clone(), dim));

    vec![
        Line::from(Span::styled(clip(&location, usize::from(width)), dim)),
        Line::from(spans),
    ]
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
