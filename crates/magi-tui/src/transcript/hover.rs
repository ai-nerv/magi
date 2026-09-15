//! Lighting up the fold handle under the pointer.

use super::*;

/// Light up the fold handle at `column` of `line`, if the pointer is over one.
///
/// Only the handle: the name chip looks identical but a click on it does nothing. Returns whether
/// anything changed, so a caller can skip a redraw when the pointer moved within the same chip.
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
        // Two chips of five columns each — `[ ⧉ ]` and `[ ▸ ]` — and nothing either side.
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
        // It looks identical to a handle, so lighting it would promise a click that does nothing.
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
        // Most of the screen, so the answer has to be cheap and it has to be "no".
        let mut prose = Line::from("  just some words");
        assert!((0..40).all(|column| !hovered(&mut prose, column)));
    }
}
