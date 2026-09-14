//! The model card's graphs: ratatui's own charts, drawn into a buffer off screen and read back as
//! rows, so a scrolling float holds them like any other line.

use ratatui::buffer::Buffer;
use ratatui::layout::{Direction, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Bar, BarChart, BarGroup, Chart, Dataset, GraphType, LineGauge, Widget,
};

/// One bar: its label, its length, what is written on it, and its colour.
pub(super) struct Item {
    pub label: String,
    pub value: u64,
    pub said: String,
    pub ink: Color,
}

/// A widget drawn into `width` × `height` cells and read back as rows of styled text, one span per
/// run of cells that share a style.
pub(super) fn rows(widget: impl Widget, width: u16, height: u16) -> Vec<Line<'static>> {
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

/// Each item as a horizontal bar out of `max`, a row apart, its figure written on its end.
pub(super) fn bars(items: &[Item], max: u64, width: u16) -> Vec<Line<'static>> {
    let bars: Vec<Bar<'_>> = items
        .iter()
        .map(|item| {
            Bar::default()
                .value(item.value)
                .label(Line::from(item.label.clone()))
                .text_value(item.said.clone())
                .style(Style::default().fg(item.ink))
                .value_style(
                    Style::default()
                        .fg(item.ink)
                        .add_modifier(Modifier::REVERSED),
                )
        })
        .collect();
    let chart = BarChart::default()
        .direction(Direction::Horizontal)
        .bar_width(1)
        .bar_gap(1)
        .max(max.max(1))
        .data(BarGroup::default().bars(&bars));
    let height = u16::try_from(items.len() * 2)
        .unwrap_or(u16::MAX)
        .saturating_sub(1);
    rows(chart, width, height)
}

/// Each item as an upright column, the newest that fit, its figure at its top and its label below.
pub(super) fn columns(items: &[Item], width: u16, height: u16) -> Vec<Line<'static>> {
    let fit = usize::from(width / 5).max(1);
    let shown = &items[items.len().saturating_sub(fit)..];
    let bars: Vec<Bar<'_>> = shown
        .iter()
        .map(|item| {
            Bar::default()
                .value(item.value)
                .label(Line::from(item.label.clone()))
                .text_value(item.said.clone())
                .style(Style::default().fg(item.ink))
                .value_style(
                    Style::default()
                        .fg(item.ink)
                        .add_modifier(Modifier::REVERSED),
                )
        })
        .collect();
    let chart = BarChart::default()
        .bar_width(4)
        .bar_gap(1)
        .data(BarGroup::default().bars(&bars));
    rows(chart, width, height)
}

/// A line through `points`, drawn in braille over axes: turns along, `0..top` up, `ticks` its marks.
pub(super) fn line(
    points: &[(f64, f64)],
    top: f64,
    ticks: [String; 3],
    ink: Color,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let axis = Style::default().fg(crate::colour::dim());
    let last = points.last().map_or(1.0, |(x, _)| *x).max(2.0);
    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(ink))
        .data(points);
    let [low, middle, high] = ticks;
    let chart = Chart::new(vec![dataset])
        .x_axis(
            Axis::default()
                .bounds([1.0, last])
                .style(axis)
                .labels(vec![Line::from("1"), Line::from(format!("{last:.0}"))]),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, top.max(f64::EPSILON)])
                .style(axis)
                .labels(vec![Line::from(low), Line::from(middle), Line::from(high)]),
        );
    rows(chart, width, height)
}

/// A thick line filled `ratio` of the way, with `label` before it.
pub(super) fn gauge(ratio: f64, label: String, ink: Color, width: u16) -> Vec<Line<'static>> {
    let gauge = LineGauge::default()
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label)
        .filled_style(Style::default().fg(ink))
        .unfilled_style(Style::default().fg(crate::colour::border()))
        .line_set(symbols::line::THICK);
    rows(gauge, width, 1)
}
