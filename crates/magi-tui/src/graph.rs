//! What this project is made of, and what depends on what.
//!
//! **The shell, deliberately.** What goes in here is a code index — files, the functions in them,
//! and the edges between — and building one is a real piece of work with a real dependency
//! (a parser per language). This is the shape it will arrive into: a view that says what it holds,
//! a ranking that is the first thing worth drawing, and an empty state that says how to fill it.
//!
//! **A ranking, not a picture.** The obvious drawing is nodes and edges, and it is the wrong one
//! for a terminal: it needs sub-cell resolution to look like anything, and the one thing a
//! terminal surface may not assume is that a cell is divisible. A ranked bar chart of *coupling*
//! answers the question people actually open this for — "if I change this, what else moves" —
//! in glyphs a terminal has, at any width.
//!
//! **Where the index will come from.** Not from magi: a code index is a thing you run over a
//! tree and keep, which is a store and a parser and a watch on the filesystem. That is a sibling's
//! worth of ownership, and the family already has a shape for a sibling that answers questions.
//! Until it exists, [`Graph::empty`] says so rather than drawing an empty box.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One thing in the project, and how entangled it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// What it is called — a path, or a path and a symbol.
    pub name: String,
    /// How many other things reach it, or it reaches.
    pub edges: usize,
}

/// What the project looks like.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// Everything indexed, in whatever order it was built.
    pub nodes: Vec<Node>,
}

/// The bar glyphs, eighth-block, so a bar is accurate to an eighth of a cell.
///
/// The whole sub-cell budget a terminal surface gets, spent here. Anything finer needs braille,
/// which does not respect the cell grid.
const BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

impl Graph {
    /// Nothing indexed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything has been indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// What to say when nothing has been indexed.
    ///
    /// A sentence that names the command rather than the problem: somebody who opened this
    /// wanted to see the project, and "no index" tells them what is missing rather than what to
    /// do about it.
    #[must_use]
    pub fn empty() -> String {
        "no index yet — run `:graph init` to build one".to_owned()
    }

    /// The coupling ranking, `width` columns wide, most entangled first.
    ///
    /// Ranked rather than drawn: high coupling is the blast radius of an edit, which is the
    /// question this is opened for.
    #[must_use]
    pub fn ranked(&self, width: usize) -> Vec<Line<'static>> {
        if self.nodes.is_empty() {
            return Vec::new();
        }
        let mut nodes = self.nodes.clone();
        nodes.sort_by(|a, b| b.edges.cmp(&a.edges).then_with(|| a.name.cmp(&b.name)));

        let widest = nodes.iter().map(|n| n.edges).max().unwrap_or(1).max(1);
        let label = nodes
            .iter()
            .map(|n| n.name.chars().count())
            .max()
            .unwrap_or(0)
            .min(40);
        // What is left for the bar, once the name and the count have had theirs.
        let room = width.saturating_sub(label + 8).max(1);

        nodes
            .iter()
            .map(|node| {
                Line::from(vec![
                    Span::raw(format!("{:<label$}  ", cut(&node.name, label))),
                    Span::raw(bar(node.edges, widest, room)),
                    Span::styled(
                        format!("  {}", node.edges),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ])
            })
            .collect()
    }
}

/// A bar `room` columns wide at most, for `value` out of `most`.
///
/// Eighth-blocks, so a value that is a third of the widest reads as a third rather than rounding
/// to the nearest whole cell — which at eight columns is a quarter of the chart.
fn bar(value: usize, most: usize, room: usize) -> String {
    if most == 0 || room == 0 {
        return String::new();
    }
    let eighths = value * room * 8 / most;
    // **Something rather than nothing, for a value that is not nothing.** One edge out of a
    // hundred rounds to zero eighths, and an empty bar beside the count `1` reads as a bug in
    // the chart. The thinnest glyph is the honest answer: present, and nearly nothing.
    if eighths == 0 {
        return if value > 0 {
            BLOCKS[0].to_string()
        } else {
            String::new()
        };
    }
    let full = eighths / 8;
    let rest = eighths % 8;
    let mut out = "█".repeat(full.min(room));
    if rest > 0 && full < room {
        out.push(BLOCKS[rest - 1]);
    }
    out
}

/// `text`, cut to `width` characters.
fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    // From the left, because a path's tail is the part that identifies it.
    let keep: String = text
        .chars()
        .skip(text.chars().count().saturating_sub(width - 1))
        .collect();
    format!("…{keep}")
}

#[cfg(test)]
#[path = "graph/ranking.rs"]
mod ranking;
