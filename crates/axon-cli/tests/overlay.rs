//! The completion popup, on screen.

use axon_tui::complete::{self, Kind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn no_paths(_: &str) -> Vec<String> {
    Vec::new()
}

fn some_paths(_: &str) -> Vec<String> {
    ["src/main.rs", "src/lib.rs", "src/app.rs", "Cargo.toml"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

fn screen(line: &str, col: usize, width: u16, paths: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
    let completion = complete::resolve(line, col, paths).expect("a popup");
    let lines = complete::render(&completion, width);
    let mut terminal =
        Terminal::new(TestBackend::new(width, lines.len() as u16)).expect("test terminal");
    terminal
        .draw(|frame| {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(lines.clone()),
                frame.area(),
            );
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn the_command_palette_marks_the_selection_and_shows_details() {
    let rendered = screen("/", 1, 50, &no_paths);
    assert_eq!(
        rendered,
        vec![
            "❯ /help         show keybindings and commands",
            "  /clear        start a fresh conversation",
            "  /model        the model, or /model <name> to swi",
            "  /permissions  ask the model what it needs, and d",
            "  /resume       continue a session from this direc",
            "  /rewind       undo the last exchange, or /rewind",
            "  /think        how much reasoning to ask for",
            "  /quit         exit axon",
        ]
    );
}

#[test]
fn typing_narrows_the_palette_to_one_row() {
    let rendered = screen("/cl", 3, 50, &no_paths);
    assert_eq!(rendered.len(), 1);
    assert!(rendered[0].starts_with("❯ /clear"), "{rendered:?}");
}

#[test]
fn path_completion_ranks_the_closest_match_first() {
    let rendered = screen("open @main", 10, 40, &some_paths);
    assert!(rendered[0].starts_with("❯ src/main.rs"), "{rendered:?}");
}

#[test]
fn the_popup_never_exceeds_its_row_budget() {
    let many: Vec<String> = (0..40).map(|i| format!("file{i}.rs")).collect();
    let list = |_: &str| many.clone();
    let completion = complete::resolve("@file", 5, &list).expect("a popup");
    assert_eq!(completion.kind, Kind::Path);
    assert_eq!(
        usize::from(completion.height()),
        complete::rows(),
        "a long list is capped, not scrolled off screen"
    );
}

#[test]
fn details_share_one_column_regardless_of_value_length() {
    let rendered = screen("/", 1, 50, &no_paths);
    let columns: Vec<usize> = rendered
        .iter()
        .map(|row| {
            // Every row opens with a two-cell marker; the detail column is the second run of
            // whitespace, not the first.
            let body = &row[row.char_indices().nth(2).map_or(0, |(i, _)| i)..];
            let gap = body.find("  ").expect("a gap before the detail");
            let detail = body[gap..]
                .find(|c: char| !c.is_whitespace())
                .expect("a detail");
            gap + detail
        })
        .collect();
    assert!(
        columns.windows(2).all(|w| w[0] == w[1]),
        "details start at differing columns: {columns:?}"
    );
}
