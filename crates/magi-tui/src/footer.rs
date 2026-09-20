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

/// A model as the footer says it: an initial for who serves it, an initial for whose model it is,
/// and then the model itself.
///
/// `openrouter/deepseek/deepseek-v4-flash-0731` reads `o/d/deepseek-v4-flash-0731`. The two
/// leading parts are routing — which door it came through, and whose name is on it — and a
/// letter apiece is enough to tell one configured model from another. The last part is the
/// answer to "which model is this", so it is left whole.
#[must_use]
pub fn short_model(name: &str) -> String {
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
    // Nothing to shorten: one part is the model and nothing else, and two is already short.
    let Some((model, routing)) = parts.split_last().filter(|(_, lead)| !lead.is_empty()) else {
        return name.to_owned();
    };
    let initials = routing
        .iter()
        .filter_map(|part| part.chars().next())
        .map(|first| first.to_lowercase().to_string())
        .collect::<Vec<_>>()
        .join("/");
    format!("{initials}/{model}")
}

/// The model's slot: whatever the name and the gaps around the middle leave. Asked here by the
/// draw and by both layouts, so the button, its cells and what is drawn cannot disagree.
fn fit_model(model: &str, inset: usize, name: &str) -> String {
    let gap = usize::from(crate::metric::column_gap());
    fit_path(
        &short_model(model),
        inset.saturating_sub(name.chars().count() + gap * 2),
    )
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
    let name = fit_name(&data.identity, inset);
    let model = fit_model(&data.model, inset, &name);
    let end = pad + u16::try_from(inset).unwrap_or(u16::MAX);
    end.saturating_sub(u16::try_from(model.chars().count()).unwrap_or(0))..end
}

/// Where a middle `said` cells wide starts, in columns of the inset `width`, when it fits between
/// the name and the model; `None` when it is left out. The draw and the pointer's layout both ask
/// here, so the dots and the cells a hover is measured against agree.
fn middle_start(data: &FooterData, said: usize, width: usize) -> Option<usize> {
    let gap = usize::from(crate::metric::column_gap());
    let name = fit_name(&data.identity, width);
    let model = fit_model(&data.model, width, &name);
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
/// How wide one sibling's segment is, `[✻ MEL]`, and how far the next one starts from it.
pub const SIBLING_WIDTH: u16 = 7;
pub const SIBLING_STEP: u16 = 8;

/// `[✻ MEL] [✻ BAL] [✻ CAS]`: a star each, in the footer's own colour at rest and red for a sibling
/// that is down; the one whose menu is open drawn inverted, the way the name shows it is a button.
/// `stirred` is how lit each one is this frame: a lit one turns to a `●` in its sibling's own hue,
/// so a flickering sibling changes shape as well as colour.
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
        let rest = if up[nth] {
            colour::dim()
        } else {
            colour::error()
        };
        let (glyph, ink) = match stirred[nth] {
            by if by > 0.5 => ("●", colour::blend(rest, stir_hue(nth), by)),
            _ => ("✻", rest),
        };
        spans.push(Span::styled("[", dim.add_modifier(lit)));
        spans.push(Span::styled(
            glyph,
            Style::default().fg(ink).add_modifier(lit),
        ));
        spans.push(Span::styled(format!(" {short}]"), dim.add_modifier(lit)));
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

    /// Whether a span is one of the three marks, at rest or lit.
    fn marks(span: &Span<'_>) -> bool {
        span.content == "✻" || span.content == "●"
    }

    #[test]
    fn a_lit_segment_is_one_background_dot_included() {
        // The whole segment is dim reversed, the dot with it, so its ground matches the rest.
        let lit = siblings([true; 3], Some(0), [0.0; 3]);
        for piece in lit.iter().take(3) {
            assert_eq!(piece.style.fg, Some(colour::dim()), "{piece:?}");
            assert!(piece.style.add_modifier.contains(Modifier::REVERSED));
        }
    }

    #[test]
    fn three_segments_one_dot_each_at_fixed_columns() {
        let drawn = text(&siblings([true, false, true], None, [0.0; 3]));
        assert_eq!(drawn, "[✻ MEL] [✻ BAL] [✻ CAS]");
        for (nth, (short, _)) in SIBLINGS.iter().enumerate() {
            let at = usize::from(SIBLING_STEP) * nth;
            let segment: String = drawn
                .chars()
                .skip(at)
                .take(usize::from(SIBLING_WIDTH))
                .collect();
            assert_eq!(segment, format!("[✻ {short}]"));
        }
    }

    #[test]
    fn a_dot_rests_in_the_footer_colour_and_is_red_only_when_down() {
        let dots: Vec<_> = siblings([true, false, true], None, [0.0; 3])
            .into_iter()
            .filter(|s| s.content == "✻")
            .map(|s| s.style.fg)
            .collect();
        assert_eq!(
            dots,
            vec![
                Some(colour::dim()),
                Some(colour::error()),
                Some(colour::dim())
            ]
        );
    }

    #[test]
    fn only_the_dot_of_a_stirred_sibling_flashes_its_own_hue() {
        let drawn = |stirred: [f32; 3]| siblings([true; 3], None, stirred);
        let dot = |stirred| {
            drawn(stirred)
                .into_iter()
                .filter(marks)
                .nth(2)
                .map(|s| (s.content.into_owned(), s.style.fg))
        };
        assert_eq!(
            dot([0.0, 0.0, 1.0]),
            Some((
                "●".to_owned(),
                Some(colour::blend(colour::dim(), colour::warning(), 1.0))
            )),
            "a lit one is a dot in its own hue"
        );
        assert_eq!(
            dot([1.0, 1.0, 0.0]),
            Some(("✻".to_owned(), Some(colour::dim()))),
            "only its own"
        );
        assert!(
            drawn([1.0; 3])
                .iter()
                .filter(|s| !marks(s))
                .all(|s| s.style.fg == Some(colour::dim())),
            "the brackets and the name stay as they are"
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

    // Ends first, shorter of the two with priority: the name takes its third of the inset, the model
    // whatever is left once the name and the gaps around the middle are out.
    let name = fit_name(&data.identity, width);
    let model = fit_model(&data.model, width, &name);
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

#[cfg(test)]
#[path = "footer/tests.rs"]
mod tests;

/// The control for moving between agents, and the width it is allowed to cost.
#[cfg(test)]
#[path = "footer/crewing.rs"]
mod crewing;
