//! Lighting up the handle under the pointer.
//!
//! Split out under THE RULE; the transcript next door is what this is about.

use super::*;

/// Light up the fold handle at `column` of `line`, if the pointer is over one.
///
/// Reversed, which is the same thing a selection does to the text under it: the terminal already
/// means "this is the thing you are pointing at" by inverting, and a second idea — a brighter
/// colour, a bolder weight — would be a second vocabulary for one meaning.
///
/// **Only the handle.** The name chip is set into the edge the same way and looks identical, and
/// lighting it up would promise a click that does nothing. What makes a chip a handle is the
/// glyph inside it, so that is what is asked.
///
/// Returns whether anything changed, so a caller can leave the screen alone when the pointer
/// moved within the same chip — which, with motion reported per cell, is most of the time.
pub fn hovered(line: &mut Line<'static>, column: u16) -> bool {
    let handles = [glyph::expand(), glyph::collapse(), glyph::copy()];
    let mut at = 0_u16;
    for span in &mut line.spans {
        let wide = u16::try_from(span.content.chars().count()).unwrap_or(0);
        let under = (at..at.saturating_add(wide)).contains(&column);
        if under && handles.iter().any(|glyph| span.content.contains(glyph)) {
            span.style = span.style.add_modifier(Modifier::REVERSED);
            return true;
        }
        at = at.saturating_add(wide);
    }
    false
}

/// What lights up under the pointer, and what does not.
#[cfg(test)]
mod pointing {
    use super::*;

    /// The top edge of a tool block, which wears a handle and a name.
    fn edge() -> Line<'static> {
        entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "shell".into(),
                args: "{}".into(),
                result: Some(magi_proto::ToolResult {
                    output: "out".into(),
                    is_error: false,
                    shown: None,
                }),
                thought_signature: None,
            },
            60,
            Detail::Preview,
        )
        .into_iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains(glyph::expand()))
        })
        .expect("a top edge")
    }

    /// Every column of `line` that inverts when the pointer is on it.
    fn lit(line: &Line<'static>) -> Vec<u16> {
        (0..80)
            .filter(|column| hovered(&mut line.clone(), *column))
            .collect()
    }

    #[test]
    fn the_handle_lights_up_and_the_edge_around_it_does_not() {
        let line = edge();
        let lit = lit(&line);
        // Two chips of five columns each — `[ ⧉ ]` and `[ ▸ ]` — and nothing either side of
        // them. Both act on a click, so both answer the pointer.
        assert_eq!(lit.len(), 10, "{lit:?}");
        // Contiguous within each chip, with the edge between them dark.
        let breaks = lit.windows(2).filter(|pair| pair[1] != pair[0] + 1).count();
        assert_eq!(breaks, 1, "one gap, between the two chips: {lit:?}");
        let first = *lit.first().expect("something lit");
        assert!(!hovered(&mut line.clone(), first.saturating_sub(1)));
        assert!(!hovered(&mut line.clone(), lit[lit.len() - 1] + 1));
    }

    #[test]
    fn the_name_chip_does_not_light_up() {
        // It is set into the edge the same way and looks identical, so lighting it would promise
        // a click that does nothing.
        let line = edge();
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let at = text
            .find("shell")
            .and_then(|at| u16::try_from(at).ok())
            .expect("the name");
        assert!(!hovered(&mut line.clone(), at), "the name lit up");
    }

    #[test]
    fn a_lit_handle_is_reversed_and_nothing_else_is() {
        let line = edge();
        let column = *lit(&line).first().expect("something lit");
        let mut under = line.clone();
        assert!(hovered(&mut under, column));
        let reversed = under
            .spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::REVERSED))
            .count();
        assert_eq!(reversed, 1, "exactly the chip under the pointer");
    }

    #[test]
    fn a_row_with_no_handle_on_it_lights_nothing() {
        // Most of the screen. The answer has to be cheap and it has to be "no".
        let mut prose = Line::from("  just some words");
        assert!((0..40).all(|column| !hovered(&mut prose, column)));
    }
}
