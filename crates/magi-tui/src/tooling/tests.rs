use super::*;

fn tool(name: &str, group: &str, needs: Option<&str>) -> Tool {
    Tool {
        name: name.to_owned(),
        group: group.to_owned(),
        needs: needs.map(ToOwned::to_owned),
        deferred: false,
        about: "does a thing".to_owned(),
    }
}

fn text_of(drawn: &Rendered) -> String {
    drawn
        .rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn held<'a>(tools: Option<&'a [Tool]>, calls: &'a [(String, usize)]) -> Held<'a> {
    Held {
        tools,
        calls,
        width: 78,
    }
}

#[test]
fn every_tab_draws_something_rather_than_panicking_with_nothing_to_show() {
    for tab in 0..TABS.len() {
        let drawn = view(&held(None, &[]), tab);
        assert_eq!(drawn.rows.len(), drawn.picks.len(), "tab {tab}");
        assert!(!empty(tab, true).is_empty(), "tab {tab}");
    }
}

#[test]
fn tools_are_gathered_under_their_branch_of_the_manual() {
    // A flat run of twenty names says nothing about what the session can do; the branches do.
    // Interleaved, the way the program actually lists them: a heading raised whenever the group
    // changes prints `files` twice and says the session has two sets of file tools.
    let tools = [
        tool("read", "files", Some("read")),
        tool("sese", "finding", Some("read")),
        tool("edit", "files", Some("write")),
    ];
    let text = text_of(&view(&held(Some(&tools), &[]), 0));
    let heads: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("  ") && !line.trim().is_empty())
        .collect();
    assert_eq!(heads, ["files", "finding"], "{text}");
}

#[test]
fn a_tool_that_belongs_to_no_branch_still_has_a_home() {
    let tools = [tool("odd", "", None)];
    assert!(text_of(&view(&held(Some(&tools), &[]), 0)).contains("other"));
}

#[test]
fn the_permission_a_tool_acts_under_is_shown_beside_it() {
    let tools = [tool("write", "files", Some("write"))];
    assert!(text_of(&view(&held(Some(&tools), &[]), 0)).contains("write"));
    let free = [tool("tools", "manual", None)];
    assert!(text_of(&view(&held(Some(&free), &[]), 0)).contains('—'));
}

#[test]
fn the_counts_are_barred_against_the_busiest_rather_than_a_fixed_scale() {
    // A bar against a fixed maximum is flat for a quiet session and clipped for a busy one.
    let calls = vec![("read".to_owned(), 10), ("edit".to_owned(), 5)];
    let drawn = view(&held(None, &calls), 1);
    let bars: Vec<usize> = drawn
        .rows
        .iter()
        .take(2)
        .map(|row| row.to_string().matches('▂').count())
        .collect();
    assert!(bars[0] > bars[1] && bars[1] > 0, "{bars:?}");
}

#[test]
fn the_calls_tab_says_how_many_there_were_in_all() {
    let calls = vec![("read".to_owned(), 3), ("shell".to_owned(), 2)];
    let text = text_of(&view(&held(None, &calls), 1));
    assert!(text.contains("5 calls, 2 tools"), "{text}");
}

#[test]
fn one_is_never_told_in_the_plural() {
    let once = vec![("read".to_owned(), 1)];
    let text = text_of(&view(&held(None, &once), 1));
    assert!(text.contains("(1 call, 1 tool)"), "{text}");
}

#[test]
fn a_session_that_has_called_nothing_says_so_rather_than_drawing_a_blank() {
    assert!(view(&held(None, &[]), 1).rows.is_empty());
    assert!(empty(1, true).contains("no tool has been called"));
}

#[test]
fn a_list_not_yet_answered_reads_differently_from_an_empty_one() {
    assert!(empty(0, false).contains("asking"));
    assert!(!empty(0, true).contains("asking"), "{}", empty(0, true));
}
