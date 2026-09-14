//! The model card's graphs, drawn cell by cell so they read smooth: bars in eighth-blocks on a
//! dotted track, columns rising from a floor like a level meter, and a line over a filled area in
//! braille. Everything comes back as rows, so a scrolling float holds them like any other line.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Widget};

/// One bar or column: its label, its size, what is written beside it, and its colour.
pub(super) struct Item {
    pub label: String,
    pub value: f64,
    pub said: String,
    pub ink: Color,
}

/// The cell a bar ends in, by the eighths of it that are filled.
const ACROSS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
/// The cell a column tops out in, by the eighths of it that are filled.
const UP: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// `ink` at `t` of its strength: dark where a bar starts, bright where it ends.
fn fade(ink: Color, t: f32) -> Color {
    crate::colour::blend(crate::colour::blend(ink, Color::Rgb(0, 0, 0), 0.55), ink, t)
}

/// `part` of `whole`, for a gradient.
fn portion(part: usize, whole: usize) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a cell count, far below where f32 loses precision"
    )]
    let share = part as f32 / whole.max(1) as f32;
    share
}

/// `fraction` of `of` cells, rounded, never more than all of them.
fn cells(fraction: f64, of: usize) -> usize {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "clamped to 0..=1 of a cell count"
    )]
    let filled = (fraction.clamp(0.0, 1.0) * of as f64).round() as usize;
    filled.min(of)
}

/// A widget drawn into `width` × `height` cells and read back as rows of styled text, one span per
/// run of cells that share a style.
fn rows(widget: impl Widget, width: u16, height: u16) -> Vec<Line<'static>> {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    widget.render(area, &mut buffer);
    (0..height)
        .map(|y| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for x in 0..width {
                let cell = &buffer[(x, y)];
                let style = cell.style();
                match spans.last_mut() {
                    Some(last) if last.style == style => {
                        last.content.to_mut().push_str(cell.symbol());
                    }
                    _ => spans.push(Span::styled(cell.symbol().to_owned(), style)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// Each item as a bar out of `max`, a row each: its label, the bar on a dotted track, its figure.
pub(super) fn bars(items: &[Item], max: f64, width: u16) -> Vec<Line<'static>> {
    let label = items
        .iter()
        .map(|item| item.label.chars().count())
        .max()
        .unwrap_or(0)
        .min(16);
    let said = items
        .iter()
        .map(|item| item.said.chars().count())
        .max()
        .unwrap_or(0);
    let room = usize::from(width).saturating_sub(label + said + 4).max(4);
    let track = Style::default().fg(crate::colour::border());
    items
        .iter()
        .map(|item| {
            let eighths = cells(item.value / max.max(f64::EPSILON), room * 8);
            let (full, part) = (eighths / 8, eighths % 8);
            let used = full + usize::from(part > 0);
            let name: String = item.label.chars().take(label).collect();
            let mut spans = vec![Span::styled(
                format!("{name:<label$}  "),
                Style::default().fg(crate::colour::muted()),
            )];
            for at in 0..full {
                let ink = fade(item.ink, portion(at + 1, used));
                spans.push(Span::styled("█", Style::default().fg(ink)));
            }
            if part > 0 {
                spans.push(Span::styled(ACROSS[part], Style::default().fg(item.ink)));
            }
            spans.push(Span::styled("·".repeat(room.saturating_sub(used)), track));
            spans.push(Span::styled(
                format!(" {:>said$}", item.said),
                Style::default().fg(item.ink),
            ));
            Line::from(spans)
        })
        .collect()
}

/// Each item as a column rising from a floor, the newest that fit, the peak marked on the left and
/// the first and last labels under them. Brighter towards the top, the way a level meter is.
pub(super) fn columns(items: &[Item], width: u16, height: u16) -> Vec<Line<'static>> {
    let top = items.iter().map(|item| item.value).fold(0.0_f64, f64::max);
    let peak = items
        .iter()
        .max_by(|a, b| a.value.total_cmp(&b.value))
        .map_or_else(String::new, |item| item.said.clone());
    let gutter = peak.chars().count().max(1);
    let fit = (usize::from(width).saturating_sub(gutter + 2) / 3).max(1);
    let shown = &items[items.len().saturating_sub(fit)..];
    let height = usize::from(height).max(2);
    let axis = Style::default().fg(crate::colour::border());
    let scale = Style::default().fg(crate::colour::dim());
    let mut out = Vec::with_capacity(height + 2);
    for row in 0..height {
        let floor = (height - 1 - row) * 8;
        let mark = if row == 0 { peak.as_str() } else { "" };
        let mut spans = vec![
            Span::styled(format!("{mark:>gutter$}"), scale),
            Span::styled("│", axis),
        ];
        for item in shown {
            let filled = cells(item.value / top.max(f64::EPSILON), height * 8);
            let here = UP[filled.saturating_sub(floor).min(8)];
            let ink = fade(item.ink, portion(height - row, height));
            spans.push(Span::styled(
                format!(" {here}{here}"),
                Style::default().fg(ink),
            ));
        }
        out.push(Line::from(spans));
    }
    out.push(Line::from(vec![
        Span::styled(format!("{:>gutter$}", "0"), scale),
        Span::styled(format!("└{}", "─".repeat(shown.len() * 3)), axis),
    ]));
    if let (Some(first), Some(last)) = (shown.first(), shown.last()) {
        let left = format!("turn {}", first.label);
        let right = if shown.len() > 1 {
            format!("turn {}", last.label)
        } else {
            String::new()
        };
        let gap = (shown.len() * 3).saturating_sub(left.chars().count() + right.chars().count());
        out.push(Line::from(Span::styled(
            format!("{}{left}{}{right}", " ".repeat(gutter + 1), " ".repeat(gap)),
            scale,
        )));
    }
    out
}

/// `label`, then a bar filled `ratio` of the way along a track, in `ink`.
pub(super) fn gauge(ratio: f64, label: &str, ink: Color, width: u16) -> Vec<Line<'static>> {
    let room = usize::from(width)
        .saturating_sub(label.chars().count() + 1)
        .max(4);
    let eighths = cells(ratio, room * 8);
    let (full, part) = (eighths / 8, eighths % 8);
    let mut spans = vec![Span::styled(
        format!("{label} "),
        Style::default().fg(crate::colour::muted()),
    )];
    for at in 0..full {
        let lit = fade(ink, portion(at + 1, full));
        spans.push(Span::styled("█", Style::default().fg(lit)));
    }
    if part > 0 {
        spans.push(Span::styled(ACROSS[part], Style::default().fg(ink)));
    }
    spans.push(Span::styled(
        "─".repeat(room.saturating_sub(full + usize::from(part > 0))),
        Style::default().fg(crate::colour::border()),
    ));
    vec![Line::from(spans)]
}

/// A line through `points` over a filled area beneath it, in braille, on quiet axes: turns along,
/// `0..top` up, `ticks` its marks.
pub(super) fn line(
    points: &[(f64, f64)],
    top: f64,
    ticks: [String; 3],
    ink: Color,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let axis = Style::default().fg(crate::colour::border());
    let dim = Style::default().fg(crate::colour::dim());
    let last = points.last().map_or(1.0, |(x, _)| *x).max(2.0);
    let dense = resampled(points, usize::from(width) * 2);
    let area = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Bar)
        .style(Style::default().fg(fade(ink, 0.2)))
        .data(&dense);
    let edge = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(ink))
        .data(points);
    let [low, middle, high] = ticks;
    let chart = Chart::new(vec![area, edge])
        .x_axis(Axis::default().bounds([1.0, last]).style(axis).labels(vec![
            Line::styled("1", dim),
            Line::styled(format!("{last:.0}"), dim),
        ]))
        .y_axis(
            Axis::default()
                .bounds([0.0, top.max(f64::EPSILON)])
                .style(axis)
                .labels(vec![
                    Line::styled(low, dim),
                    Line::styled(middle, dim),
                    Line::styled(high, dim),
                ]),
        );
    rows(chart, width, height)
}

/// `points` resampled to `samples` evenly along, straight between each, so the area under them is
/// solid rather than a comb of one stroke a turn.
fn resampled(points: &[(f64, f64)], samples: usize) -> Vec<(f64, f64)> {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    if points.len() < 2 {
        return points.to_vec();
    }
    let steps = f64::from(u32::try_from(samples.max(2) - 1).unwrap_or(1));
    (0..samples)
        .map(|n| {
            let x = first.0 + (last.0 - first.0) * f64::from(u32::try_from(n).unwrap_or(0)) / steps;
            let after = points
                .iter()
                .position(|point| point.0 >= x)
                .unwrap_or(points.len() - 1)
                .max(1);
            let (a, b) = (points[after - 1], points[after]);
            let t = if (b.0 - a.0).abs() < f64::EPSILON {
                0.0
            } else {
                (x - a.0) / (b.0 - a.0)
            };
            (x, a.1 + (b.1 - a.1) * t)
        })
        .collect()
}
