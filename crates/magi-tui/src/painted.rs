//! What a tool *meant*, drawn in magi's own colours.
//!
//! casper's tools never choose a colour. They name a role — `added`, `keyword`, `path` — and this
//! is where that becomes a colour, out of the same `magi.ui` palette the prompt box and the footer
//! use. It is the whole reason a `patch` and a syntax-highlighted `cat` agree on screen: both
//! land here, and one palette paints them.
//!
//! Every role resolves to a colour that *already exists*. Nothing new was added for this: a diff's
//! roles are the four `diff_*` colours a tool block has always drawn a patch with, so the same
//! change looks the same whether it came from `edit` or from casper. Adding a colour per role
//! would have made those two disagree the moment somebody set a theme.
//!
//! A role this build has no name for reads as [`Role::Text`] on the way in — see the contract —
//! so there is no case here for "unknown", and a newer casper's vocabulary degrades to plain
//! text rather than to a panic.

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
        // A heading is the one thing on a screen of output worth finding first, which is what
        // the accent is for everywhere else.
        Role::Title => colour::accent(),
        // A path is a thing you can go and look at, which is what inline code already
        // means everywhere else on the screen.
        Role::Path => colour::md_code(),
        Role::Ok => colour::success(),
        Role::Warn => colour::warning(),
        Role::Error => colour::error(),
        // The four a tool block already draws a patch with, so a diff from casper and a diff
        // from `edit` are the same diff.
        Role::Added => colour::diff_added(),
        Role::Removed => colour::diff_removed(),
        Role::Marker => colour::diff_marker(),
        Role::Context => colour::diff_context(),
        // Code. Borrowed from the markdown palette rather than given colours of their own: a
        // highlighted `cat` and a fenced block in an answer are the same code on the same
        // screen, and two sets of colours for that would read as two languages.
        Role::Keyword => colour::md_heading(),
        Role::String => colour::md_code(),
        Role::Number => colour::md_code(),
        Role::Comment => colour::md_quote(),
        Role::Type => colour::md_code_block(),
        Role::Func => colour::accent(),
    }
}

/// One painted line, as the renderer draws it.
#[must_use]
pub fn line(spans: &[Span]) -> Line<'static> {
    Line::from(
        spans
            .iter()
            .map(|span| {
                ratatui::text::Span::styled(span.text.clone(), Style::default().fg(of(span.role)))
            })
            .collect::<Vec<_>>(),
    )
}

/// A whole painted document.
#[must_use]
pub fn lines(painted: &[Vec<Span>]) -> Vec<Line<'static>> {
    painted.iter().map(|spans| line(spans)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_from_a_tool_is_the_same_diff_a_block_already_drew() {
        // The claim the whole design rests on. If these ever diverge, one change looks like two
        // depending on which tool produced it — which is exactly what casper exists to end.
        assert_eq!(of(Role::Added), colour::diff_added());
        assert_eq!(of(Role::Removed), colour::diff_removed());
        assert_eq!(of(Role::Marker), colour::diff_marker());
        assert_eq!(of(Role::Context), colour::diff_context());
    }

    #[test]
    fn every_role_resolves_to_a_colour_the_palette_already_had() {
        // Nothing new was added for casper. A colour per role would drift from the ones a tool
        // block draws with, and the two would disagree on the first theme somebody set.
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
            colour::md_heading(),
            colour::md_code(),
            colour::md_quote(),
            colour::md_code_block(),
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
        // A renderer that lost a character while colouring it would be worse than one that drew
        // everything grey.
        let drawn = line(&[
            Span::new(Role::Removed, "-was"),
            Span::new(Role::Text, "  "),
            Span::new(Role::Added, "+now"),
        ]);
        let text: String = drawn.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "-was  +now");
    }

    #[test]
    fn each_span_keeps_its_own_colour() {
        let drawn = line(&[Span::new(Role::Added, "+a"), Span::new(Role::Removed, "-b")]);
        assert_eq!(drawn.spans[0].style.fg, Some(colour::diff_added()));
        assert_eq!(drawn.spans[1].style.fg, Some(colour::diff_removed()));
    }

    #[test]
    fn a_blank_line_is_still_a_line() {
        // Lines map one to one, or every line number after a blank one is wrong.
        let drawn = lines(&[
            vec![Span::new(Role::Text, "one")],
            vec![Span::new(Role::Text, "")],
            vec![Span::new(Role::Text, "two")],
        ]);
        assert_eq!(drawn.len(), 3);
    }
}
