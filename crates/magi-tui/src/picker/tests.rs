use super::*;
use ratatui::text::Line;

#[test]
fn model_names_longer_than_the_old_cap_remain_complete_in_narrow_tables() {
    let name = "openrouter/provider/this-is-a-long-model-name-with-a-variant";
    assert!(name.len() > 48);
    let mut picker = Picker::new(
        "Model",
        vec![Choice {
            value: name.into(),
            detail: "128k\t$0.3\t$1.2\t#7\ttext,image→text\tlow,medium,high".into(),
            ready: true,
        }],
        None,
    );
    picker.table(&[
        "Model", "Ctx", "In$/M", "Out$/M", "ORwk", "Modality", "Thinking",
    ]);
    for width in [76, 180] {
        let shown = text(&render(&picker, width));
        assert_eq!(shown.len(), 3);
        assert!(shown[2].contains(name), "{}", shown[2]);
    }
}

#[test]
fn a_table_has_one_row_per_model_and_aligned_columns_at_every_width() {
    let choices = (0..30)
        .map(|index| Choice {
            value: format!("openrouter/provider/long-model-name-{index}"),
            detail: "128k\t$0.3\t$1.2\t#7\ttext,image,video→text\tmax,xhigh,high,medium,low".into(),
            ready: index % 2 == 0,
        })
        .collect();
    let mut picker = Picker::new("Model", choices, None);
    picker.table(&[
        "Model", "Ctx", "In$/M", "Out$/M", "ORwk", "Modality", "Thinking",
    ]);
    for _ in 0..25 {
        picker.next();
    }
    for width in [40, 76, 120, 180] {
        let rendered = render(&picker, width);
        assert_eq!(rendered.len(), picker.window().len() + 2);
        assert_eq!(rendered.len(), usize::from(picker.height()));
        assert!(
            rendered
                .iter()
                .all(|line| line.width() <= usize::from(width))
        );
        assert!(rendered.iter().all(|line| !line.to_string().contains('\n')));
    }
    let shown = text(&render(&picker, 180));
    assert!(shown[0].contains('↑') && shown[0].contains('↓'));
    let header = &shown[1];
    for row in &shown[2..] {
        for (title, value) in [
            ("Ctx", "128k"),
            ("In$/M", "$0.3"),
            ("Out$/M", "$1.2"),
            ("ORwk", "#7"),
            ("Modality", "text,image,video→text"),
            ("Thinking", "max,xhigh,high,medium,low"),
        ] {
            let position = |line: &str, value: &str| {
                line.find(value).map(|index| line[..index].chars().count())
            };
            assert_eq!(
                position(header, title),
                position(row, value),
                "{header}\n{row}"
            );
        }
    }
    let narrow = text(&render(&picker, 76));
    for (row, choice) in narrow[2..].iter().zip(&picker.choices[picker.window()]) {
        assert!(row.contains(&choice.value), "{row}");
        assert!(row.contains("128k"), "{row}");
    }
    picker.push('2');
    assert!(picker.current().expect("current").value.contains('2'));
    picker.push('!');
    assert!(picker.choices.is_empty());
    assert_eq!(render(&picker, 76).len(), usize::from(picker.height()));
}

#[test]
fn multiline_details_scroll_with_the_highlight_and_report_the_rendered_height() {
    let choices = (0..30).map(|index| Choice {
        value: format!("p/model-{index}"),
        detail: "128k · in $0.3 out $1.2 USD/M\ninput: text,image → output: text\nthinking: low,medium,high".into(),
        ready: true,
    }).collect();
    let mut picker = Picker::new("Model", choices, None);
    picker.details_below();
    for _ in 0..25 {
        picker.next();
    }
    let rendered = render(&picker, 80);
    assert_eq!(rendered.len(), usize::from(picker.height()));
    assert!(rendered.len() <= max_visible() + 1);
    let shown = text(&rendered).join("\n");
    assert!(shown.contains("p/model-25"), "{shown}");
    assert!(
        shown.contains("input: text,image → output: text"),
        "{shown}"
    );
    assert!(shown.contains("thinking: low,medium,high"), "{shown}");
    assert!(shown.contains('↑') && shown.contains('↓'), "{shown}");
}

fn choices() -> Vec<Choice> {
    vec![
        Choice {
            value: "a/one".into(),
            detail: "131k".into(),
            ready: true,
        },
        Choice {
            value: "b/two".into(),
            detail: "set B_KEY".into(),
            ready: false,
        },
        Choice {
            value: "c/three".into(),
            detail: "200k".into(),
            ready: true,
        },
    ]
}

fn text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn it_opens_on_what_is_already_chosen() {
    // A list of forty opened at row one makes you find where you are before you can move.
    let picker = Picker::new("Model", choices(), Some("c/three"));
    assert_eq!(picker.current().expect("a row").value, "c/three");
}

#[test]
fn it_opens_at_the_top_when_nothing_is_chosen() {
    let picker = Picker::new("Model", choices(), None);
    assert_eq!(picker.current().expect("a row").value, "a/one");
}

#[test]
fn it_opens_at_the_top_when_the_choice_is_not_in_the_list() {
    // A model set in config that the catalog no longer has, which is exactly the state
    // somebody is in when they come looking for this list.
    let picker = Picker::new("Model", choices(), Some("gone/away"));
    assert_eq!(picker.current().expect("a row").value, "a/one");
}

#[test]
fn the_highlight_wraps_both_ways() {
    let mut picker = Picker::new("Model", choices(), None);
    picker.previous();
    assert_eq!(picker.current().expect("a row").value, "c/three");
    picker.next();
    assert_eq!(picker.current().expect("a row").value, "a/one");
}

#[test]
fn an_unusable_choice_is_shown_with_what_it_needs() {
    // Filtering it out leaves an empty list for somebody who has configured nothing, and
    // an empty list does not tell them which variable to set.
    let shown = text(&render(&Picker::new("Model", choices(), None), 60));
    assert!(shown.iter().any(|l| l.contains("b/two")), "{shown:?}");
    assert!(shown.iter().any(|l| l.contains("set B_KEY")), "{shown:?}");
}

#[test]
fn the_heading_says_where_you_are_in_the_list() {
    let shown = text(&render(&Picker::new("Model", choices(), None), 60));
    assert!(shown[0].contains("Model"), "{:?}", shown[0]);
    assert!(shown[0].contains("1 of 3"), "{:?}", shown[0]);
}

#[test]
fn a_long_list_scrolls_to_keep_the_highlight_on_screen() {
    let many: Vec<Choice> = (0..30)
        .map(|i| Choice {
            value: format!("m/{i:02}"),
            detail: "1k".into(),
            ready: true,
        })
        .collect();
    let mut picker = Picker::new("Model", many, None);
    for _ in 0..20 {
        picker.next();
    }
    let shown = text(&render(&picker, 60));
    assert!(shown.iter().any(|l| l.contains("m/20")), "{shown:?}");
    assert_eq!(shown.len(), max_visible() + 1, "heading plus a window");
}

#[test]
fn every_row_fills_the_width() {
    let picker = Picker::new("Model", choices(), None);
    for line in render(&picker, 40) {
        let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(width, 40, "{line:?}");
    }
}

#[test]
fn detail_rows_keep_metadata_visible_and_selection_inside_the_window() {
    let choices = (0..20)
        .map(|index| Choice {
            value: format!("openrouter/vendor/a-long-model-name-{index}"),
            detail: "128k · in $0.3 out $1.2 USD/M · OR wk #7".into(),
            ready: true,
        })
        .collect();
    let mut picker = Picker::new(
        "Model",
        choices,
        Some("openrouter/vendor/a-long-model-name-19"),
    );
    picker.details_below();
    let shown = render(&picker, 80);
    assert_eq!(shown.len(), usize::from(picker.height()));
    assert!(shown.len() <= max_visible() + 1);
    let text = shown
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("a-long-model-name-19"), "{text}");
    assert!(text.contains("in $0.3 out $1.2 USD/M · OR wk #7"), "{text}");
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    fn many() -> Vec<Choice> {
        [
            "openrouter/deepseek/deepseek-v3.2",
            "openrouter/anthropic/claude-sonnet-4.5",
            "anthropic/claude-haiku-4-5",
            "ollama/llama3.3",
        ]
        .iter()
        .map(|name| Choice {
            value: (*name).to_owned(),
            detail: "1k".into(),
            ready: true,
        })
        .collect()
    }

    fn values(picker: &Picker) -> Vec<&str> {
        picker.choices.iter().map(|c| c.value.as_str()).collect()
    }

    #[test]
    fn typing_narrows_the_list() {
        // Fifty-three rows is more than anyone should arrow through.
        let mut picker = Picker::new("Model", many(), None);
        for c in "sonnet".chars() {
            picker.push(c);
        }
        assert_eq!(
            values(&picker),
            vec!["openrouter/anthropic/claude-sonnet-4.5"]
        );
    }

    #[test]
    fn backspacing_widens_it_again() {
        // Destructive filtering would mean a typo could only be undone by closing the list.
        let mut picker = Picker::new("Model", many(), None);
        for c in "sonnetx".chars() {
            picker.push(c);
        }
        assert!(picker.is_empty(), "the typo matches nothing");
        picker.pop();
        assert_eq!(
            values(&picker),
            vec!["openrouter/anthropic/claude-sonnet-4.5"]
        );
    }

    #[test]
    fn clearing_the_query_offers_everything_again() {
        let mut picker = Picker::new("Model", many(), None);
        picker.push('z');
        while picker.pop() {}
        assert_eq!(picker.choices.len(), 4);
    }

    #[test]
    fn narrowing_puts_the_highlight_back_at_the_top() {
        // The old index was a position in a different list, and keeping it lands on whatever
        // happens to be there now.
        let mut picker = Picker::new("Model", many(), None);
        picker.next();
        picker.next();
        picker.push('l');
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn a_query_matching_nothing_says_so_rather_than_going_blank() {
        let mut picker = Picker::new("Model", many(), None);
        for c in "zzzz".chars() {
            picker.push(c);
        }
        let shown: Vec<String> = render(&picker, 60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(shown[0].contains("nothing matches"), "{shown:?}");
        assert!(shown[0].contains("zzzz"), "{shown:?}");
    }

    #[test]
    fn a_query_matching_nothing_is_not_the_same_as_an_empty_catalog() {
        // One you can back out of; the other is a configuration with no providers in it.
        let mut picker = Picker::new("Model", many(), None);
        picker.push('z');
        assert!(picker.is_empty());
        assert!(!picker.offers_nothing());
        assert!(Picker::new("Model", Vec::new(), None).offers_nothing());
    }

    #[test]
    fn the_heading_shows_what_was_typed() {
        let mut picker = Picker::new("Model", many(), None);
        picker.push('l');
        let shown: Vec<String> = render(&picker, 60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(shown[0].contains("▸ l"), "{:?}", shown[0]);
    }
}

#[cfg(test)]
mod take_tests {
    use super::*;

    fn mixed() -> Vec<Choice> {
        vec![
            Choice {
                value: "ready/one".into(),
                detail: "1k".into(),
                ready: true,
            },
            Choice {
                value: "locked/two".into(),
                detail: "set TWO_KEY".into(),
                ready: false,
            },
        ]
    }

    #[test]
    fn a_usable_row_is_taken() {
        let mut picker = Picker::new("Model", mixed(), None);
        assert_eq!(picker.take(), Some("ready/one".to_owned()));
    }

    #[test]
    fn a_locked_row_is_refused_without_closing_anything() {
        // Picking it can only fail, and the failure is written on the row. A round trip to be
        // told that would close the list to say it, and then you retype the query.
        let mut picker = Picker::new("Model", mixed(), None);
        picker.next();
        assert_eq!(picker.take(), None);
        let heading: String = render(&picker, 70)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(heading.contains("set TWO_KEY"), "{heading}");
    }

    #[test]
    fn moving_on_clears_what_was_said_about_the_last_one() {
        let mut picker = Picker::new("Model", mixed(), None);
        picker.next();
        let _ = picker.take();
        picker.previous();
        let heading: String = render(&picker, 70)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!heading.contains("TWO_KEY"), "{heading}");
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    fn mixed() -> Vec<Choice> {
        vec![
            Choice {
                value: "openai/gpt-5".into(),
                detail: "set OPENAI_API_KEY".into(),
                ready: false,
            },
            Choice {
                value: "openai/gpt-5.1".into(),
                detail: "set OPENAI_API_KEY".into(),
                ready: false,
            },
            Choice {
                value: "openrouter/openai/gpt-5.1".into(),
                detail: "400k".into(),
                ready: true,
            },
            Choice {
                value: "local/llama".into(),
                detail: "131k".into(),
                ready: true,
            },
        ]
    }

    #[test]
    fn a_filter_puts_what_you_can_run_first() {
        // Typing `gpt` used to rank five rows needing an unset key above the one usable match.
        let mut picker = Picker::new("Model", mixed(), None);
        for c in "gpt".chars() {
            picker.push(c);
        }
        assert!(
            picker.current().expect("a row").ready,
            "the top row is usable"
        );
        assert_eq!(
            picker.current().expect("a row").value,
            "openrouter/openai/gpt-5.1"
        );
    }

    #[test]
    fn the_highlight_lands_somewhere_enter_will_work() {
        let mut picker = Picker::new("Model", mixed(), None);
        for c in "gpt".chars() {
            picker.push(c);
        }
        assert_eq!(picker.take(), Some("openrouter/openai/gpt-5.1".to_owned()));
    }

    #[test]
    fn opening_with_no_current_value_skips_the_unusable() {
        let picker = Picker::new("Model", mixed(), None);
        assert!(
            picker.current().expect("a row").ready,
            "not opened on a refusal"
        );
    }

    #[test]
    fn a_current_value_still_wins_over_readiness() {
        // Where you already are outranks the rule: you opened the list to move from it.
        let picker = Picker::new("Model", mixed(), Some("openai/gpt-5.1"));
        assert_eq!(picker.current().expect("a row").value, "openai/gpt-5.1");
    }

    #[test]
    fn a_filter_matching_nothing_usable_still_shows_the_reasons() {
        let none_ready = vec![
            Choice {
                value: "openai/gpt-5".into(),
                detail: "set OPENAI_API_KEY".into(),
                ready: false,
            },
            Choice {
                value: "azure/gpt-5".into(),
                detail: "set AZURE_OPENAI_API_KEY".into(),
                ready: false,
            },
        ];
        let mut picker = Picker::new("Model", none_ready, None);
        picker.push('g');
        assert!(!picker.is_empty(), "the rows are still listed");
        assert!(picker.take().is_none(), "and taking one says why instead");
        assert!(picker.notice.is_some());
    }
}

#[cfg(test)]
mod fluidity_tests {
    use super::*;
    use crate::colour;
    use ratatui::text::Line;

    fn many(n: usize) -> Vec<Choice> {
        (1..=n)
            .map(|i| Choice {
                value: format!("provider/model-{i}"),
                detail: "131k".into(),
                ready: true,
            })
            .collect()
    }

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_long_list_says_how_much_is_below() {
        // Fifty rows in a window of eight gave no sign there was anything past the eighth, so
        // moving through it felt like the list was changing under you.
        let picker = Picker::new("Model", many(50), None);
        let shown = text(&render(&picker, 60));
        assert!(shown[0].contains('↓'), "{:?}", shown[0]);
        assert!(
            !shown[0].contains('↑'),
            "nothing is above the top: {:?}",
            shown[0]
        );
    }

    #[test]
    fn scrolling_down_says_how_much_is_above() {
        let mut picker = Picker::new("Model", many(50), None);
        for _ in 0..40 {
            picker.next();
        }
        let shown = text(&render(&picker, 60));
        assert!(shown[0].contains('↑'), "{:?}", shown[0]);
    }

    #[test]
    fn a_list_that_fits_says_neither() {
        let picker = Picker::new("Model", many(3), None);
        let shown = text(&render(&picker, 60));
        assert!(
            !shown[0].contains('↑') && !shown[0].contains('↓'),
            "{:?}",
            shown[0]
        );
    }

    #[test]
    fn the_highlight_bar_reaches_the_edge() {
        // A background that stops where the text stops reads as a ragged block, which is worse
        // than no highlight at all.
        let picker = Picker::new("Model", many(5), None);
        let rendered = render(&picker, 60);
        let row = &rendered[1];
        let painted: usize = row.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(painted, 60, "the selected row fills the width");
        assert!(
            row.spans.iter().any(|s| s.style.bg.is_some()),
            "and it is filled"
        );
    }

    #[test]
    fn an_unselected_row_paints_nothing_and_the_selected_one_paints_a_bar() {
        // The list is drawn inside the prompt box, so the box is what says where it starts and
        // stops. All a row has to say for itself is whether it is the one you are on.
        let picker = Picker::new("Model", many(5), None);
        let rendered = render(&picker, 60);
        assert!(
            rendered[2]
                .spans
                .iter()
                .all(|s| s.style.bg == Some(ratatui::style::Color::Reset)),
            "nothing behind it"
        );
        assert!(
            rendered[1]
                .spans
                .iter()
                .any(|s| s.style.bg == Some(colour::menu_selected_bg())),
            "and the bar on the row above it"
        );
    }
}
