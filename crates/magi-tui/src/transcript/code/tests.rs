//! A file's language, found and drawn; an edit's changes, each on its own ground.
//!
//! Split out under THE RULE; the recolouring these check is next door.

use super::*;
use magi_proto::tooling::Role;

fn painted(texts: &[&str], role: Role) -> Vec<Vec<Painted>> {
    texts
        .iter()
        .map(|text| vec![Painted::new(role, *text)])
        .collect()
}

fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn ink(line: &Line<'_>, piece: &str) -> Option<Color> {
    line.spans
        .iter()
        .find(|s| s.content == piece)
        .and_then(|s| s.style.fg)
}

#[test]
fn a_file_that_was_read_is_highlighted_in_its_language() {
    let shown = repaint(
        "read",
        r#"{"path":"src/a.rs"}"#,
        &painted(&["fn main() {}"], Role::Text),
        Style::default(),
    )
    .expect("rust is known");
    assert_eq!(ink(&shown[0].0, "fn"), Some(colour::code_keyword()));
    assert_eq!(text(&shown[0].0), "fn main() {}");
}

#[test]
fn an_unknown_file_or_another_tool_keeps_caspers_painting() {
    let lines = painted(&["x"], Role::Text);
    let unknown = repaint("read", r#"{"path":"notes.zzz"}"#, &lines, Style::default());
    assert!(unknown.is_none());
    let other = repaint("shell", r#"{"path":"a.rs"}"#, &lines, Style::default());
    assert!(other.is_none());
}

#[test]
fn a_write_keeps_its_heading_above_the_file() {
    let mut lines = vec![vec![
        Painted::new(Role::Path, "a.py"),
        Painted::new(Role::Dim, "  1 lines · new file"),
    ]];
    lines.extend(painted(&["def f(): return 1"], Role::Text));
    let shown =
        repaint("write", r#"{"path":"a.py"}"#, &lines, Style::default()).expect("python is known");
    assert_eq!(text(&shown[0].0), "a.py  1 lines · new file");
    assert_eq!(ink(&shown[1].0, "def"), Some(colour::code_keyword()));
}

#[test]
fn an_edit_puts_each_change_on_the_ground_of_what_happened_to_it() {
    let diff = [
        "--- a.rs",
        "+++ a.rs",
        "@@ -1,3 +1,4 @@",
        " let a = 1;",
        "-let b = 2;",
        "+let b = 3;",
        "+let c = 4;",
        " let d = 5;",
        "+let e = 6;",
    ];
    let shown = repaint(
        "edit",
        r#"{"path":"a.rs"}"#,
        &painted(&diff, Role::Context),
        Style::default(),
    )
    .expect("rust is known");
    let ground: Vec<Option<Color>> = shown.iter().map(|(_, on)| on.bg).collect();
    let (added, removed, changed) = (
        Some(colour::diff_added_bg()),
        Some(colour::diff_removed_bg()),
        Some(colour::diff_changed_bg()),
    );
    assert_eq!(
        ground,
        [
            None, None, None, None, removed, changed, changed, None, added
        ]
    );
    for (row, source) in shown.iter().zip(diff) {
        assert_eq!(text(&row.0), source, "nothing lost or added");
    }
    assert_eq!(
        ink(&shown[5].0, "let"),
        Some(colour::code_keyword()),
        "the code keeps its syntax on the ground"
    );
}
