//! What `read`, `write` and `edit` show, highlighted in the language of the file they touched: the
//! code in syntax colours, and an edit's lines on a green, red or orange ground for added, removed
//! and changed, so the change and the code both read at once.

use crate::colour;
use magi_proto::tooling::Span as Painted;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// The file a call touched, as the token syntect finds a language by: its extension, or its whole
/// name for one like `Makefile` that has none.
fn language(name: &str, args: &str) -> Option<String> {
    if !matches!(name, "read" | "write" | "edit") {
        return None;
    }
    let serde_json::Value::Object(fields) = serde_json::from_str(args).ok()? else {
        return None;
    };
    let file = fields.get("path")?.as_str()?.rsplit('/').next()?;
    Some(
        file.rsplit_once('.')
            .map_or(file, |(_, ext)| ext)
            .to_owned(),
    )
}

/// The text of a painted line, whatever roles it was cut into.
fn text_of(line: &[Painted]) -> String {
    line.iter().map(|span| span.text.as_str()).collect()
}

/// Each line of what the call showed, recoloured, with the style its row is drawn on. `None` when
/// it is not one of the three, or its language is one syntect does not know: casper's painting stands.
pub(super) fn repaint(
    name: &str,
    args: &str,
    lines: &[Vec<Painted>],
    on: Style,
) -> Option<Vec<(Line<'static>, Style)>> {
    let language = language(name, args)?;
    if name == "edit" {
        return diff(&language, lines, on);
    }
    // `write` heads the file with where it went: that line is casper's, the rest is the file.
    let head = usize::from(name == "write" && !lines.is_empty());
    let texts: Vec<String> = lines[head..].iter().map(|line| text_of(line)).collect();
    let pieces = crate::syntax::pieces(&language, &texts)?;
    let mut out: Vec<(Line<'static>, Style)> = lines[..head]
        .iter()
        .map(|line| (crate::painted::line(line, on), on))
        .collect();
    out.extend(pieces.into_iter().map(|line| (coloured(line, on), on)));
    Some(out)
}

fn coloured(pieces: Vec<(Color, String)>, on: Style) -> Line<'static> {
    Line::from(
        pieces
            .into_iter()
            .map(|(fg, text)| Span::styled(text, on.fg(fg)))
            .collect::<Vec<_>>(),
    )
}

/// What a line of a diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Heading,
    Context,
    Added,
    Removed,
    /// The new side of a change: added straight after lines that were removed.
    Changed,
}

/// A diff, its code highlighted and each changed line on the ground of what happened to it.
fn diff(language: &str, lines: &[Vec<Painted>], on: Style) -> Option<Vec<(Line<'static>, Style)>> {
    let texts: Vec<String> = lines.iter().map(|line| text_of(line)).collect();
    let mut after_removed = false;
    let kinds: Vec<Change> = texts
        .iter()
        .map(|text| {
            let kind = if ["+++", "---", "@@"]
                .iter()
                .any(|head| text.starts_with(head))
            {
                Change::Heading
            } else if text.starts_with('+') && after_removed {
                Change::Changed
            } else if text.starts_with('+') {
                Change::Added
            } else if text.starts_with('-') {
                Change::Removed
            } else {
                Change::Context
            };
            after_removed = matches!(kind, Change::Removed | Change::Changed);
            kind
        })
        .collect();
    // The code without its column of marks, highlighted as one run so what spans lines stays coloured.
    let code: Vec<String> = texts
        .iter()
        .zip(&kinds)
        .filter(|(_, kind)| **kind != Change::Heading)
        .map(|(text, _)| text.get(1..).unwrap_or_default().to_owned())
        .collect();
    let mut pieces = crate::syntax::pieces(language, &code)?.into_iter();
    let drawn = lines
        .iter()
        .zip(&texts)
        .zip(&kinds)
        .map(|((painted, text), kind)| {
            let (row, mark) = match kind {
                Change::Heading => return (crate::painted::line(painted, on), on),
                Change::Context => (on, colour::diff_context()),
                Change::Added => (on.bg(colour::diff_added_bg()), colour::diff_added()),
                Change::Removed => (on.bg(colour::diff_removed_bg()), colour::diff_removed()),
                Change::Changed => (on.bg(colour::diff_changed_bg()), colour::warning()),
            };
            let mut spans = vec![Span::styled(
                text.get(..1).unwrap_or_default().to_owned(),
                row.fg(mark),
            )];
            spans.extend(
                pieces
                    .next()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(fg, piece)| Span::styled(piece, row.fg(fg))),
            );
            (Line::from(spans), row)
        })
        .collect();
    Some(drawn)
}

#[cfg(test)]
#[path = "code/tests.rs"]
mod tests;
