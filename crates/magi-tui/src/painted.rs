//! What a tool meant, drawn in magi's own colours.
//!
//! casper's tools name a role — `added`, `keyword`, `path` — and this resolves it against the same
//! `magi.ui` palette the prompt box and footer use, so a `patch` and a highlighted `cat` agree.
//! Every role maps to a colour that already exists. A role this build has no name for arrives as
//! [`Role::Text`], so there is no "unknown" case here.

use crate::colour;
use magi_proto::tooling::{Role, Span};
use ratatui::style::{Color, Style};
use ratatui::text::Line;

/// The colour a role is drawn in.
#[must_use]
pub fn of(role: Role) -> Color {
    match role {
        Role::Text => colour::text(),
        Role::Muted => colour::muted(),
        Role::Dim => colour::dim(),
        Role::Title => colour::accent(),
        Role::Path => colour::md_code(),
        Role::Ok => colour::success(),
        Role::Warn => colour::warning(),
        Role::Error => colour::error(),
        // The four a tool block already draws a patch with.
        Role::Added => colour::diff_added(),
        Role::Removed => colour::diff_removed(),
        Role::Marker => colour::diff_marker(),
        Role::Context => colour::diff_context(),
        Role::Changed => colour::warning(),
        // The code colours, so a file casper highlighted and a fenced block agree.
        Role::Keyword => colour::code_keyword(),
        Role::String => colour::code_string(),
        Role::Number => colour::code_number(),
        Role::Comment => colour::code_comment(),
        Role::Type => colour::code_type(),
        Role::Func => colour::code_command(),
    }
}

/// One painted line, as the renderer draws it.
///
/// `on` is the style of whatever is drawing it, and only the foreground is replaced: a role is a
/// colour of text and never a background, or a painted row punches holes in the block it sits in.
#[must_use]
pub fn line(spans: &[Span], on: Style) -> Line<'static> {
    Line::from(
        spans
            .iter()
            .map(|span| {
                // A colour asked for outright wins over the role; only a surface does this.
                let fg = span
                    .rgb
                    .map_or_else(|| of(span.role), |[r, g, b]| Color::Rgb(r, g, b));
                let style = match (span.back.and_then(ground_of), span.bg) {
                    (Some(ground), _) => on.fg(fg).bg(ground),
                    (None, Some([r, g, b])) => on.fg(fg).bg(Color::Rgb(r, g, b)),
                    (None, None) => on.fg(fg),
                };
                ratatui::text::Span::styled(span.text.clone(), style)
            })
            .collect::<Vec<_>>(),
    )
}

/// The ground a change sits on, from the palette; `None` for a role that is not a change.
#[must_use]
pub fn ground_of(role: Role) -> Option<Color> {
    match role {
        Role::Added => Some(colour::diff_added_bg()),
        Role::Removed => Some(colour::diff_removed_bg()),
        Role::Changed => Some(colour::diff_changed_bg()),
        _ => None,
    }
}

/// What a whole row of `spans` is drawn on: `on`, on the ground of the change it marks, if any, so
/// the colour runs the width of the block rather than stopping where the text does.
#[must_use]
pub fn row(spans: &[Span], on: Style) -> Style {
    spans
        .first()
        .and_then(|span| span.back)
        .and_then(ground_of)
        .map_or(on, |ground| on.bg(ground))
}

/// A whole painted document.
#[must_use]
pub fn lines(painted: &[Vec<Span>], on: Style) -> Vec<Line<'static>> {
    painted.iter().map(|spans| line(spans, on)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_from_a_tool_is_the_same_diff_a_block_already_drew() {
        assert_eq!(of(Role::Added), colour::diff_added());
        assert_eq!(of(Role::Removed), colour::diff_removed());
        assert_eq!(of(Role::Marker), colour::diff_marker());
        assert_eq!(of(Role::Context), colour::diff_context());
    }

    #[test]
    fn every_role_resolves_to_a_colour_the_palette_already_had() {
        let known = [
            colour::text(),
            colour::muted(),
            colour::dim(),
            colour::accent(),
            colour::success(),
            colour::warning(),
            colour::error(),
            colour::diff_added(),
            colour::diff_removed(),
            colour::diff_marker(),
            colour::diff_context(),
            colour::md_code(),
            colour::code_keyword(),
            colour::code_string(),
            colour::code_number(),
            colour::code_comment(),
            colour::code_type(),
            colour::code_command(),
        ];
        for role in [
            Role::Text,
            Role::Muted,
            Role::Dim,
            Role::Title,
            Role::Path,
            Role::Ok,
            Role::Warn,
            Role::Error,
            Role::Added,
            Role::Removed,
            Role::Marker,
            Role::Context,
            Role::Keyword,
            Role::String,
            Role::Number,
            Role::Comment,
            Role::Type,
            Role::Func,
        ] {
            assert!(known.contains(&of(role)), "{role:?} invented a colour");
        }
    }

    #[test]
    fn the_text_survives_whatever_the_roles_do() {
        let drawn = line(
            &[
                Span::new(Role::Removed, "-was"),
                Span::new(Role::Text, "  "),
                Span::new(Role::Added, "+now"),
            ],
            Style::default(),
        );
        let text: String = drawn.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "-was  +now");
    }

    #[test]
    fn each_span_keeps_its_own_colour() {
        let drawn = line(
            &[Span::new(Role::Added, "+a"), Span::new(Role::Removed, "-b")],
            Style::default(),
        );
        assert_eq!(drawn.spans[0].style.fg, Some(colour::diff_added()));
        assert_eq!(drawn.spans[1].style.fg, Some(colour::diff_removed()));
    }

    #[test]
    fn a_blank_line_is_still_a_line() {
        // Lines map one to one, or every line number after a blank one is wrong.
        let drawn = lines(
            &[
                vec![Span::new(Role::Text, "one")],
                vec![Span::new(Role::Text, "")],
                vec![Span::new(Role::Text, "two")],
            ],
            Style::default(),
        );
        assert_eq!(drawn.len(), 3);
    }
}
