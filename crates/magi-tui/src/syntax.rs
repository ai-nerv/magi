//! Code, highlighted. syntect reads the language from Sublime's regex grammars — nothing compiled
//! per language — and the colours are this palette's rather than a theme's, so a code block matches
//! the shell commands and the rest of the screen.

use crate::colour;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as Ink, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::SyntaxSet;

/// How many highlighted blocks are remembered. The transcript is drawn every frame, and a block
/// that has not changed need not be parsed again.
const KEPT: usize = 256;

/// Each line of a block as the colour and text of its pieces.
pub(crate) type Pieces = Vec<Vec<(Color, String)>>;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Each kind of token in a colour of the palette; syntect takes the most specific scope that matches.
fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let rules = [
            ("comment", colour::code_comment()),
            ("string, constant.character", colour::code_string()),
            (
                "constant.numeric, constant.language, constant.other",
                colour::code_variable(),
            ),
            ("keyword, storage", colour::code_keyword()),
            ("keyword.operator", colour::code_operator()),
            (
                "entity.name.function, support.function, variable.function, entity.name.tag",
                colour::code_command(),
            ),
            (
                "entity.name.type, entity.name.class, support.type, support.class",
                colour::code_type(),
            ),
            (
                "entity.other.attribute-name, variable.parameter",
                colour::code_flag(),
            ),
        ];
        Theme {
            settings: ThemeSettings {
                foreground: ink(colour::text()),
                ..ThemeSettings::default()
            },
            scopes: rules
                .iter()
                .filter_map(|(scope, colour)| {
                    Some(ThemeItem {
                        scope: scope.parse().ok()?,
                        style: StyleModifier {
                            foreground: ink(*colour),
                            background: None,
                            font_style: None,
                        },
                    })
                })
                .collect(),
            ..Theme::default()
        }
    })
}

/// A palette colour as syntect writes one; `None` for one with no fixed RGB value.
fn ink(colour: Color) -> Option<Ink> {
    match colour::blend(colour, colour, 0.0) {
        Color::Rgb(r, g, b) => Some(Ink { r, g, b, a: 0xff }),
        _ => None,
    }
}

fn remembered() -> &'static Mutex<HashMap<(String, String), Pieces>> {
    static HELD: OnceLock<Mutex<HashMap<(String, String), Pieces>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The lines of a fenced block in `language`, each as spans on `base`: highlighted where syntect
/// knows the language, and in the code block colour where it does not.
#[must_use]
pub fn block(language: &str, lines: &[String], base: Style) -> Vec<Vec<Span<'static>>> {
    let pieces = pieces(language, lines).unwrap_or_else(|| {
        lines
            .iter()
            .map(|line| vec![(colour::md_code_block(), line.clone())])
            .collect()
    });
    pieces
        .into_iter()
        .map(|line| {
            line.into_iter()
                .map(|(fg, text)| Span::styled(text, base.fg(fg)))
                .collect()
        })
        .collect()
}

/// The coloured pieces of each line, remembered by language and text. A block is highlighted as a
/// whole so what spans lines — a block comment, a long string — stays coloured. `None` for a
/// language syntect does not know.
pub(crate) fn pieces(language: &str, lines: &[String]) -> Option<Pieces> {
    let token = language.split_whitespace().next()?;
    let syntax = syntaxes().find_syntax_by_token(token)?;
    let key = (token.to_owned(), lines.join("\n"));
    let mut held = remembered().lock().ok()?;
    if let Some(done) = held.get(&key) {
        return Some(done.clone());
    }
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut done = Vec::with_capacity(lines.len());
    for line in lines {
        let ended = format!("{line}\n");
        let ranges = highlighter.highlight_line(&ended, syntaxes()).ok()?;
        done.push(
            ranges
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.trim_end_matches('\n');
                    let Ink { r, g, b, .. } = style.foreground;
                    (!text.is_empty()).then(|| (Color::Rgb(r, g, b), text.to_owned()))
                })
                .collect(),
        );
    }
    if held.len() >= KEPT {
        held.clear();
    }
    held.insert(key, done.clone());
    Some(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(ToOwned::to_owned).collect()
    }

    /// Every piece of the block beside its colour.
    fn painted(language: &str, text: &str) -> Vec<(String, Option<Color>)> {
        block(language, &lines(text), Style::default())
            .into_iter()
            .flatten()
            .map(|span| (span.content.into_owned(), span.style.fg))
            .collect()
    }

    #[test]
    fn a_known_language_is_coloured_by_what_each_piece_is() {
        let shown = painted("rust", "fn main() {\n    let s = \"hi\"; // why\n}");
        let colour_of = |piece: &str| {
            shown
                .iter()
                .find(|(text, _)| text.contains(piece))
                .and_then(|(_, fg)| *fg)
        };
        assert_eq!(colour_of("hi"), Some(colour::code_string()), "{shown:?}");
        // A grey reaches syntect as the RGB it stands for, and comes back as that.
        let comment = colour::blend(colour::code_comment(), colour::code_comment(), 0.0);
        assert_eq!(colour_of("why"), Some(comment), "{shown:?}");
        // `fn` and `let` are `storage.type` to the grammar, and a keyword to anybody reading.
        assert_eq!(colour_of("let"), Some(colour::code_keyword()), "{shown:?}");
        assert_ne!(colour_of("main"), colour_of("hi"), "{shown:?}");
    }

    #[test]
    fn an_unknown_language_is_the_code_block_colour() {
        for language in ["", "no-such-language"] {
            let shown = painted(language, "anything at all");
            assert_eq!(
                shown,
                [("anything at all".to_owned(), Some(colour::md_code_block()))]
            );
        }
    }

    #[test]
    fn nothing_is_lost_or_added() {
        let text = "def f(x):\n    return x * 2  # double\n\nprint(f(3))";
        let drawn: Vec<String> = block("python", &lines(text), Style::default())
            .iter()
            .map(|line| line.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(drawn, lines(text));
    }
}
