//! The footer: one dim line, the directory and branch left, usage in the middle, session name right.

use crate::colour;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// What the footer displays. The UI owns none of this; the session reports it.
#[derive(Debug, Clone, Default)]
pub struct FooterData {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_percent: Option<f64>,
    pub context_window: u64,
    /// What the agent on screen is called: this session's own `project/role/id`, or `role/id` for a
    /// peer, whose project cannot differ because a roster is one project's.
    pub identity: String,
    pub model: String,
    /// How many agents there are, this one included. No longer drawn — the agents view has taken
    /// over moving between them — but still the count the roster reports.
    pub crew: usize,
    /// Whether the agent on screen is this session. Only the identity's styling turns on it.
    pub own: bool,
    /// The pointer is over the name, which opens the agents view: drawn inverted while it is, the
    /// same block the usage badge always wears, so the name reads as the button it is.
    pub name_hover: bool,
    /// The pointer is over the model's name, which opens the model's card: inverted while it is.
    pub model_hover: bool,
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

/// Fit a path into `width`, dropping leading components rather than trailing ones: the tail is the
/// part that says where you are, and the head is what a reader can infer.
#[must_use]
pub fn fit_path(path: &str, width: usize) -> String {
    if path.chars().count() <= width {
        return path.to_owned();
    }
    // No room at all is not a licence to overflow: a caller with nothing left to give gets nothing.
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

/// The name's slot: against the left edge, a third of the inset wide. Both the draw below and the
/// layout that records where a click lands measure it here, so the button and its cells agree.
fn fit_name(identity: &str, inset: usize) -> String {
    fit_path(identity, inset / 3)
}

/// Where the name lands, as columns from the left edge (the pad included), for the click target the
/// layout records. An empty range when there is no name to aim at.
#[must_use]
pub fn name_columns(data: &FooterData, width: u16) -> std::ops::Range<u16> {
    let pad = crate::metric::footer_pad();
    let inset = usize::from(width).saturating_sub(usize::from(pad) * 2);
    let name_width = fit_name(&data.identity, inset).chars().count();
    pad..pad + u16::try_from(name_width).unwrap_or(0)
}

/// Where the model's name lands, as columns from the left edge (the pad included), for the press
/// that opens its card: right-aligned in the inset, fitted the way the draw fits it.
#[must_use]
pub fn model_columns(data: &FooterData, width: u16) -> std::ops::Range<u16> {
    let pad = crate::metric::footer_pad();
    let inset = usize::from(width).saturating_sub(usize::from(pad) * 2);
    let gap = usize::from(crate::metric::column_gap());
    let name = fit_name(&data.identity, inset);
    let model = fit_path(
        &data.model,
        inset.saturating_sub(name.chars().count() + gap * 2),
    );
    let end = pad + u16::try_from(inset).unwrap_or(u16::MAX);
    end.saturating_sub(u16::try_from(model.chars().count()).unwrap_or(0))..end
}

/// Where a middle `said` cells wide starts, in columns of the inset `width`, when it fits between
/// the name and the model; `None` when it is left out. The draw and the pointer's layout both ask
/// here, so the dots and the cells a hover is measured against agree.
fn middle_start(data: &FooterData, said: usize, width: usize) -> Option<usize> {
    let gap = usize::from(crate::metric::column_gap());
    let name = fit_name(&data.identity, width);
    let model = fit_path(
        &data.model,
        width.saturating_sub(name.chars().count() + gap * 2),
    );
    let name_width = name.chars().count();
    let model_at = width.saturating_sub(model.chars().count());
    // Centred on the row so one column changing does not slide the other two, but clamped: on a
    // narrow screen the centre reached the model and the two printed into each other.
    let middle_at = (width.saturating_sub(said) / 2)
        .max(name_width + gap)
        .min(model_at.saturating_sub(said + gap));
    (middle_at >= name_width && middle_at + said + gap <= model_at).then_some(middle_at)
}

/// Where the middle landed, as a column from the left edge (the pad included), for the pointer.
#[must_use]
pub fn middle_column(data: &FooterData, said: usize, width: u16) -> Option<u16> {
    let pad = crate::metric::footer_pad();
    let inset = usize::from(width).saturating_sub(usize::from(pad) * 2);
    middle_start(data, said, inset)
        .and_then(|at| u16::try_from(at).ok())
        .map(|at| pad + at)
}

/// The three siblings the footer reports on, in the order they are drawn: the short name on the
/// footer and the full one on the menu.
pub const SIBLINGS: [(&str, &str); 3] =
    [("MEL", "melchior"), ("BAL", "balthasar"), ("CAS", "casper")];
/// How wide one sibling's segment is, `[● MEL]`, and how far the next one starts from it.
pub const SIBLING_WIDTH: u16 = 7;
pub const SIBLING_STEP: u16 = 8;

/// `[● MEL] [● BAL] [● CAS]`: a dot each, green when that sibling is up and red when it is not; the
/// one whose menu is open drawn inverted, the way the name shows it is a button.
/// `stirred` is how lit each one still is from just having done something, 1 to 0: the dot and its
/// name flash towards that sibling's own hue and fade back.
#[must_use]
pub fn siblings(up: [bool; 3], open: Option<usize>, stirred: [f32; 3]) -> Vec<Span<'static>> {
    let dim = Style::default().fg(colour::dim());
    let mut spans = Vec::new();
    for (nth, (short, _)) in SIBLINGS.iter().enumerate() {
        if nth > 0 {
            spans.push(Span::styled(" ", dim));
        }
        let lit = if open == Some(nth) {
            Modifier::REVERSED
        } else {
            Modifier::empty()
        };
        let dot = if up[nth] {
            colour::success()
        } else {
            colour::error()
        };
        let tint = |from| match stirred[nth] {
            by if by > 0.0 => colour::blend(from, stir_hue(nth), by),
            _ => from,
        };
        let label = Style::default().fg(tint(colour::dim()));
        spans.push(Span::styled("[", label.add_modifier(lit)));
        // Not reversed: that would swap the dot's green or red into the background. The segment's
        // own background instead, with the dot still in its colour.
        let mut ink = Style::default().fg(tint(dot));
        if open == Some(nth) {
            ink = ink.bg(colour::dim());
        }
        spans.push(Span::styled("●", ink));
        spans.push(Span::styled(format!(" {short}]"), label.add_modifier(lit)));
    }
    spans
}

/// What each sibling flashes: melchior violet, balthasar cyan, casper orange.
fn stir_hue(nth: usize) -> ratatui::style::Color {
    match nth {
        0 => colour::accent(),
        1 => colour::code_type(),
        _ => colour::warning(),
    }
}

#[cfg(test)]
mod siblings_tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_lit_segment_is_one_background_dot_included() {
        // The rest is dim reversed, whose background is dim; the dot has to match it.
        let lit = siblings([true; 3], Some(0), [0.0; 3]);
        let dot = lit.iter().find(|s| s.content == "●").expect("a dot");
        assert_eq!(dot.style.bg, Some(colour::dim()));
        assert_eq!(dot.style.fg, Some(colour::success()), "still green");
        assert!(!dot.style.add_modifier.contains(Modifier::REVERSED));
        let label = lit.iter().find(|s| s.content.contains("MEL")).expect("MEL");
        assert_eq!(label.style.fg, Some(colour::dim()));
        assert!(label.style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn three_segments_one_dot_each_at_fixed_columns() {
        let drawn = text(&siblings([true, false, true], None, [0.0; 3]));
        assert_eq!(drawn, "[● MEL] [● BAL] [● CAS]");
        for (nth, (short, _)) in SIBLINGS.iter().enumerate() {
            let at = usize::from(SIBLING_STEP) * nth;
            let segment: String = drawn
                .chars()
                .skip(at)
                .take(usize::from(SIBLING_WIDTH))
                .collect();
            assert_eq!(segment, format!("[● {short}]"));
        }
    }

    #[test]
    fn a_dot_is_green_when_up_and_red_when_not() {
        let dots: Vec<_> = siblings([true, false, true], None, [0.0; 3])
            .into_iter()
            .filter(|s| s.content == "●")
            .map(|s| s.style.fg)
            .collect();
        assert_eq!(
            dots,
            vec![
                Some(colour::success()),
                Some(colour::error()),
                Some(colour::success())
            ]
        );
    }

    #[test]
    fn a_stirred_sibling_flashes_its_own_hue_and_rests_as_it_was() {
        let dot = |stirred: [f32; 3]| {
            siblings([true; 3], None, stirred)
                .into_iter()
                .filter(|s| s.content == "●")
                .nth(2)
                .and_then(|s| s.style.fg)
        };
        assert_eq!(
            dot([0.0, 0.0, 1.0]),
            Some(colour::blend(colour::success(), colour::warning(), 1.0))
        );
        assert_eq!(
            dot([1.0, 1.0, 0.0]),
            Some(colour::success()),
            "only its own"
        );
    }

    #[test]
    fn the_middle_lands_where_the_pointer_is_told() {
        let data = FooterData {
            identity: "p/lead/xi".into(),
            model: "some/model".into(),
            ..Default::default()
        };
        let middle = siblings([true; 3], None, [0.0; 3]);
        let said = middle.iter().map(|s| s.content.chars().count()).sum();
        let at = middle_column(&data, said, 120).expect("fits on a wide screen");
        let row: String = render(&data, &middle, 120)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let drawn: String = row.chars().skip(usize::from(at)).take(said).collect();
        assert_eq!(drawn, text(&middle));
    }
}

/// Render the footer, on one line: the session name on the left, usage in the middle, the model on
/// the right, each dropped in that order when the terminal cannot hold it. The name is the button
/// that opens the agents view, and inverts while the pointer is on it.
#[must_use]
pub fn render(data: &FooterData, status: &[Span<'static>], width: u16) -> Vec<Line<'static>> {
    let dim = Style::default().fg(colour::dim());
    let muted = Style::default().fg(colour::muted());
    // Held clear at both ends and the same at both, because the box above stops one short of the
    // right. Everything below measures against the inset width, not the screen.
    let pad = usize::from(crate::metric::footer_pad());
    let width = usize::from(width).saturating_sub(pad * 2);
    let gap = usize::from(crate::metric::column_gap());

    // Ends first, shorter of the two with priority: the name takes its third of the inset, the model
    // whatever is left once the name and the gaps around the middle are out.
    let name = fit_name(&data.identity, width);
    let model = fit_path(
        &data.model,
        width.saturating_sub(name.chars().count() + gap * 2),
    );
    let name_width = name.chars().count();
    let model_at = width.saturating_sub(model.chars().count());
    let said: usize = status.iter().map(|s| s.content.chars().count()).sum();

    // Brighter when it is somebody else's, inverted while the pointer is on it: everything else on
    // the screen looks the same either way, and the invert is how the name says it is a button.
    let mut name_style = if data.own { dim } else { muted };
    if data.name_hover {
        name_style = name_style.add_modifier(Modifier::REVERSED);
    }
    let mut spans = vec![Span::styled(" ".repeat(pad), dim)];
    spans.push(Span::styled(name, name_style));
    let mut col = name_width;
    if let Some(middle_at) = middle_start(data, said, width) {
        spans.push(Span::styled(" ".repeat(middle_at - col), dim));
        spans.extend(status.iter().cloned());
        col = middle_at + said;
    }
    if model_at >= col {
        spans.push(Span::styled(" ".repeat(model_at - col), dim));
    }
    let model_style = if data.model_hover {
        muted.add_modifier(Modifier::REVERSED)
    } else {
        muted
    };
    spans.push(Span::styled(model, model_style));

    let mut row = vec![spans.remove(0)];
    row.extend(clip_spans(spans, width));
    row.push(Span::styled(" ".repeat(pad), dim));
    vec![Line::from(row)]
}

/// Trim a styled line to `width`, dropping whole spans and then characters. The last guard on the
/// stats line: a line that overflows wraps, which costs the footer a row it was not given.
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
        // The status took the row above the box, which is a row of chrome for one word.
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
        // The old clip took the head, which names every directory except the one you are in.
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
        // Cut on the left by the terminal and on the right by us; either way the tail must survive.
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
            model: "claude-opus-5".into(),
            crew: 1,
            own: true,
            name_hover: false,
            model_hover: false,
        }
    }

    fn column(row: &str, needle: &str) -> Option<usize> {
        row.find(needle).map(|byte| row[..byte].chars().count())
    }

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
        // The display is fixed-width, but a middle that grows must still not push the ends.
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
        crewed(width, identity, 1)
    }

    fn crewed(width: u16, identity: &str, crew: usize) -> String {
        let data = FooterData {
            input_tokens: 12_500,
            output_tokens: 900,
            context_percent: Some(6.2),
            context_window: 200_000,
            identity: identity.into(),
            model: "claude-opus-5".into(),
            crew,
            own: true,
            name_hover: false,
            model_hover: false,
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
        // What a centred middle does once the row is inset and nobody checks the column to its right.
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

/// The usage as one string: what went up, what came down, how full the window is. Public because it
/// is worn by the prompt box now, not drawn here. Empty when there is nothing to say.
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

/// How full the context window is, and nothing else: what the prompt box's corner wears. The rest
/// of the usage is one press away, in the view the corner opens.
#[must_use]
pub fn context(data: &FooterData) -> String {
    if data.context_window == 0 {
        return String::new();
    }
    match data.context_percent {
        Some(pct) => format!("{pct:.0}%"),
        None => "?%".to_owned(),
    }
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

/// The control for moving between agents, and the width it is allowed to cost.
#[cfg(test)]
#[path = "footer/crewing.rs"]
mod crewing;
