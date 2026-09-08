//! The coupling ranking, and the glyphs it is drawn with.
//!
//! Split out under THE RULE; the index these rank is next door.

use super::*;

fn node(name: &str, edges: usize) -> Node {
    Node {
        name: name.to_owned(),
        edges,
    }
}

#[test]
fn nothing_indexed_says_how_to_index_it() {
    // Somebody who opened this wanted to see the project. "No index" names the problem; this
    // names what to do about it.
    let graph = Graph::new();
    assert!(graph.is_empty());
    assert!(graph.ranked(80).is_empty(), "no rows to draw");
    assert!(Graph::empty().contains(":graph init"), "{}", Graph::empty());
}

#[test]
fn the_most_entangled_is_first() {
    // High coupling is the blast radius of an edit, which is the question this is opened for.
    let graph = Graph {
        nodes: vec![node("small.rs", 2), node("hub.rs", 24), node("mid.rs", 9)],
    };
    let drawn: Vec<String> = graph.ranked(80).iter().map(ToString::to_string).collect();
    assert!(drawn[0].starts_with("hub.rs"), "{drawn:?}");
    assert!(drawn[1].starts_with("mid.rs"), "{drawn:?}");
    assert!(drawn[2].starts_with("small.rs"), "{drawn:?}");
}

#[test]
fn a_tie_is_broken_by_name_so_the_order_does_not_move_between_runs() {
    // A ranking that reshuffled equal rows every time it was opened would be one nobody could
    // read twice.
    let graph = Graph {
        nodes: vec![node("b.rs", 5), node("a.rs", 5), node("c.rs", 5)],
    };
    let once: Vec<String> = graph.ranked(80).iter().map(ToString::to_string).collect();
    let twice: Vec<String> = graph.ranked(80).iter().map(ToString::to_string).collect();
    assert_eq!(once, twice);
    assert!(once[0].starts_with("a.rs"), "{once:?}");
}

#[test]
fn the_widest_bar_fits_the_room_it_was_given() {
    // At any width. A bar that overflowed would wrap and the row below it would be half a chart.
    for width in [20_usize, 40, 80, 200] {
        let graph = Graph {
            nodes: vec![node("hub.rs", 100), node("leaf.rs", 1)],
        };
        for line in graph.ranked(width) {
            assert!(
                line.to_string().chars().count() <= width,
                "{} chars in {width}: {line}",
                line.to_string().chars().count()
            );
        }
    }
}

#[test]
fn a_bar_is_accurate_to_an_eighth_of_a_cell() {
    // The whole sub-cell budget a terminal surface gets. Rounding to whole cells at eight
    // columns wide is a quarter of the chart.
    assert_eq!(bar(8, 8, 8), "████████");
    assert_eq!(bar(4, 8, 8), "████");
    // A value too small for a whole cell still draws something, rather than reading as zero.
    let sliver = bar(1, 100, 8);
    assert!(!sliver.is_empty(), "one of a hundred still shows");
    assert!(sliver.chars().count() == 1, "{sliver:?}");
}

#[test]
fn nothing_to_divide_by_is_an_empty_bar_rather_than_a_panic() {
    assert_eq!(bar(0, 0, 8), "");
    assert_eq!(bar(5, 10, 0), "");
}

#[test]
fn a_long_path_keeps_the_end_that_identifies_it() {
    // Cut from the left: `crates/magi-host/src/turn/memory.rs` and
    // `crates/magi-tui/src/turn/memory.rs` differ in the part a left-cut would throw away, but
    // the tail is what a person reads.
    let cut = cut("crates/magi-host/src/supplying/packing.rs", 20);
    assert_eq!(cut.chars().count(), 20);
    assert!(cut.ends_with("packing.rs"), "{cut}");
    assert!(cut.starts_with('…'), "{cut}");
}
