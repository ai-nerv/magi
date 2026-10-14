//! Single-row choices with aligned, width-bounded columns.

use super::{Choice, Picker};
use ratatui::text::Line;

#[derive(Debug, Clone)]
pub(super) struct Columns {
    pub(super) headers: Vec<String>,
}

pub(super) struct Table<'a> {
    columns: &'a [String],
    widths: Vec<usize>,
    width: u16,
}

impl<'a> Table<'a> {
    pub(super) fn new(picker: &'a Picker, width: u16) -> Self {
        let columns = picker.columns();
        let mut widths: Vec<usize> = columns
            .iter()
            .map(|column| column.chars().count())
            .collect();
        for choice in &picker.all {
            for (index, cell) in std::iter::once(choice.value.as_str())
                .chain(choice.detail.split('\t'))
                .enumerate()
            {
                if index == 0 {
                    continue;
                }
                if let Some(width) = widths.get_mut(index) {
                    *width = (*width).max(cell.chars().count().min(48));
                }
            }
        }
        if let Some(name_width) = widths.first_mut() {
            *name_width = picker
                .choices
                .get(picker.window())
                .unwrap_or_default()
                .iter()
                .map(|choice| choice.value.chars().count())
                .max()
                .unwrap_or(0)
                .max(columns.first().map_or(0, |column| column.chars().count()));
        }
        let minimum: Vec<usize> = columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                column.chars().count().max(if index == 0 {
                    16
                } else if column == "Thinking" {
                    14
                } else if column == "Modality" {
                    12
                } else if column == "Ctx" {
                    4
                } else {
                    0
                })
            })
            .collect();
        let marker = crate::glyph::marker()
            .chars()
            .count()
            .max(crate::glyph::no_marker().chars().count());
        let gaps = columns.len().saturating_sub(1) + usize::from(columns.len() > 1);
        let available = usize::from(width).saturating_sub(marker + gaps);
        while widths.iter().sum::<usize>() > available {
            let index = widths
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(index, size)| **size > minimum.get(*index).copied().unwrap_or(0))
                .max_by_key(|(_, size)| **size)
                .map(|(index, _)| index)
                .or_else(|| {
                    widths
                        .iter()
                        .enumerate()
                        .skip(1)
                        .filter(|(_, size)| **size > 0)
                        .max_by_key(|(_, size)| **size)
                        .map(|(index, _)| index)
                });
            let Some(narrowed) = index.and_then(|index| widths.get_mut(index)) else {
                break;
            };
            *narrowed -= 1;
        }
        Self {
            columns,
            widths,
            width,
        }
    }

    pub(super) fn header(&self) -> Line<'static> {
        let cells: Vec<&str> = self.columns.iter().map(String::as_str).collect();
        self.row(&cells, false, true, "")
    }

    pub(super) fn choice(&self, choice: &Choice, selected: bool, query: &str) -> Line<'static> {
        let cells: Vec<&str> = std::iter::once(choice.value.as_str())
            .chain(choice.detail.split('\t'))
            .collect();
        self.row(&cells, selected, choice.ready, query)
    }

    fn row(&self, cells: &[&str], selected: bool, ready: bool, query: &str) -> Line<'static> {
        let formatted: Vec<String> = self
            .widths
            .iter()
            .enumerate()
            .map(|(index, width)| {
                let value = cells.get(index).copied().unwrap_or("");
                let value = if index == 0 {
                    value.to_owned()
                } else {
                    let column = self.columns.get(index).map_or("", String::as_str);
                    clip(&compact(column, value, *width), *width)
                };
                format!(
                    "{value}{}",
                    " ".repeat(width.saturating_sub(value.chars().count()))
                )
            })
            .collect();
        let value = formatted.first().map(String::as_str).unwrap_or("");
        let detail = formatted
            .iter()
            .skip(1)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        crate::menu::row(
            &crate::menu::Row {
                value,
                detail: &detail,
                selected,
                ready,
                value_width: self.widths.first().copied().unwrap_or(0).saturating_sub(1),
            },
            query,
            self.width,
        )
    }
}

fn compact(column: &str, value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    match column {
        "Modality" => value
            .split('→')
            .map(|side| {
                side.split(',')
                    .map(|modality| match modality {
                        "text" => "T",
                        "image" => "I",
                        "audio" => "A",
                        "video" => "V",
                        "file" => "F",
                        other => other,
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("→"),
        "Thinking" => value
            .split(',')
            .map(|level| match level {
                "off" | "none" => "0",
                "minimal" => "i",
                "low" => "l",
                "medium" => "m",
                "high" => "h",
                "xhigh" => "xh",
                "max" => "x",
                other => other,
            })
            .collect::<Vec<_>>()
            .join(","),
        _ => value.to_owned(),
    }
}

fn clip(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut clipped: String = value.chars().take(width - 1).collect();
    clipped.push('…');
    clipped
}
