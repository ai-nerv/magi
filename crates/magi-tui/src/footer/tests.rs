//! What the footer says, and where each part of it lands.
//!
//! Split out under THE RULE; the line these are about is next door.

use super::*;

#[cfg(test)]
mod saying {
    use super::*;

    #[test]
    fn a_model_is_an_initial_for_each_door_and_then_the_model() {
        assert_eq!(
            short_model("openrouter/deepseek/deepseek-v4-flash-0731"),
            "o/d/deepseek-v4-flash-0731"
        );
        // Two parts are a provider and a model, which is one door and the answer.
        assert_eq!(
            short_model("anthropic/claude-opus-4-6"),
            "a/claude-opus-4-6"
        );
        // One part is the answer and nothing else: there is no routing to abbreviate.
        assert_eq!(short_model("local"), "local");
        assert_eq!(short_model(""), "");
        // However many doors it came through, each is worth one letter.
        assert_eq!(short_model("a/b/c/model-1"), "a/b/c/model-1");
        // The initial is the name's own first character, lowercased, whatever case it was given.
        assert_eq!(short_model("OpenRouter/DeepSeek/Chat"), "o/d/Chat");
    }

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn token_counts_abbreviate_like_pi() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn home_collapses_to_a_tilde() {
        assert_eq!(format_cwd("/home/me", Some("/home/me")), "~");
        assert_eq!(format_cwd("/home/me/src", Some("/home/me")), "~/src");
        assert_eq!(format_cwd("/etc", Some("/home/me")), "/etc");
    }

    #[test]
    fn the_three_columns_are_the_name_the_display_and_the_model() {
        // The status took the row above the box, which is a row of chrome for one word.
        let data = FooterData {
            input_tokens: 1200,
            output_tokens: 340,
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            ..FooterData::default()
        };
        let status = [Span::raw("⣠⣾⠀⠀⠀")];
        let rendered = text_of(&render(&data, &status, 60));
        assert!(
            rendered[0].trim_start().starts_with("axum/main/alpha"),
            "the name has the left: {:?}",
            rendered[0]
        );
        assert!(
            rendered[0].contains("⣠⣾⠀⠀⠀"),
            "the display has the middle: {:?}",
            rendered[0]
        );
        assert!(
            rendered[0].trim_end().ends_with("claude-opus-5"),
            "the model has the right: {:?}",
            rendered[0]
        );
        assert!(
            !rendered[0].contains("↑1.2k"),
            "and the numbers are not here any more: {:?}",
            rendered[0]
        );
    }

    #[test]
    fn the_model_is_right_aligned() {
        let data = FooterData {
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            ..FooterData::default()
        };
        let rendered = text_of(&render(&data, &[], 60));
        assert!(
            rendered[0].trim_end().ends_with("claude-opus-5"),
            "{:?}",
            rendered[0]
        );
        // Still the full width: the model is against the inset edge, and the inset is drawn.
        assert_eq!(rendered[0].chars().count(), 60);
    }

    #[test]
    fn an_unknown_context_percentage_renders_as_a_question_mark() {
        // Worn by the prompt box now rather than the footer, but it is still this that builds it.
        let data = FooterData {
            context_window: 200_000,
            context_percent: None,
            ..FooterData::default()
        };
        assert!(usage(&data).contains("?/200k"), "{:?}", usage(&data));
    }

    #[test]
    fn no_context_window_is_no_context_group() {
        // `?/0` is three characters of noise on exactly the screen a new person is reading.
        assert_eq!(usage(&FooterData::default()), "");
    }
}
#[cfg(test)]
mod fit_tests {
    use super::*;

    fn line_text(lines: &[Line<'_>], row: usize) -> String {
        lines[row]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_long_path_keeps_the_end_that_says_where_you_are() {
        // The old clip took the head, which names every directory except the one you are in.
        let fitted = fit_path("/home/you/work/deep/nested/thing", 20);
        assert!(fitted.ends_with("thing"), "{fitted}");
        assert!(fitted.chars().count() <= 20, "{fitted}");
    }

    #[test]
    fn a_path_that_fits_is_left_alone() {
        assert_eq!(fit_path("~/work", 40), "~/work");
    }

    #[test]
    fn whole_components_survive_rather_than_half_a_word() {
        let fitted = fit_path("/aaa/bbb/ccc/ddd", 12);
        assert!(fitted.starts_with("…/"), "{fitted}");
        assert!(!fitted.contains("…/bb"), "no half components: {fitted}");
    }

    #[test]
    fn one_enormous_component_keeps_its_tail() {
        let fitted = fit_path("/x/abcdefghijklmnop", 8);
        assert!(fitted.ends_with("mnop"), "{fitted}");
        assert!(fitted.chars().count() <= 8, "{fitted}");
    }

    #[test]
    fn the_name_shows_even_with_nothing_else_to_report() {
        let data = FooterData {
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            ..FooterData::default()
        };
        let out = render(&data, &[], 60);
        assert!(
            line_text(&out, 0).contains("axum/main/alpha"),
            "{}",
            line_text(&out, 0)
        );
        assert!(
            line_text(&out, 0).contains("claude-opus-5"),
            "{}",
            line_text(&out, 0)
        );
    }
}

#[cfg(test)]
mod name_fit_tests {
    use super::*;

    fn stats_row(data: &FooterData, width: u16) -> String {
        render(data, &[], width)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_long_name_keeps_the_part_that_names_it() {
        // Cut on the left by the terminal and on the right by us; either way the tail must survive.
        let data = FooterData {
            identity: "a-long-project/main/alpha".into(),
            context_window: 164_000,
            context_percent: Some(0.0),
            ..FooterData::default()
        };
        let row = stats_row(&data, 30);
        assert!(row.contains("alpha"), "{row}");
        assert!(row.chars().count() <= 30, "{row}");
    }

    #[test]
    fn a_name_that_fits_is_left_alone() {
        let data = FooterData {
            identity: "axum/main/beta".into(),
            ..FooterData::default()
        };
        assert!(stats_row(&data, 80).contains("axum/main/beta"));
    }

    #[test]
    fn the_line_never_outgrows_the_terminal() {
        let data = FooterData {
            identity: "a-very-long-project-name/a-very-long-role/a-very-long-id".into(),
            input_tokens: 123_456,
            output_tokens: 654_321,
            context_window: 200_000,
            ..FooterData::default()
        };
        for width in [20u16, 30, 40, 60, 100] {
            let row = stats_row(&data, width);
            assert!(
                row.chars().count() <= usize::from(width),
                "width {width}: {row}"
            );
        }
    }
}

/// Each segment is placed from the width, so one changing does not move the others.
#[cfg(test)]
mod anchored {
    use super::*;

    fn data() -> FooterData {
        FooterData {
            input_tokens: 12_500,
            output_tokens: 900,
            context_percent: Some(6.2),
            context_window: 200_000,
            identity: "axum/main/alpha".into(),
            model: "claude-opus-5".into(),
            crew: 1,
            own: true,
            name_hover: false,
            model_hover: false,
        }
    }

    fn column(row: &str, needle: &str) -> Option<usize> {
        row.find(needle).map(|byte| row[..byte].chars().count())
    }

    fn row(said: &str) -> String {
        let status = [Span::raw(said.to_owned())];
        render(&data(), &status, 70)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn what_the_agent_is_doing_does_not_move_the_ends() {
        // The display is fixed-width, but a middle that grows must still not push the ends.
        let short = row("⣠⣾⠀⠀⠀");
        let long = row(&"⣿".repeat(20));
        for line in [&short, &long] {
            assert_eq!(line.chars().count(), 70, "{line:?}");
        }
        assert_eq!(
            column(&short, "axum/main/alpha"),
            column(&long, "axum/main/alpha"),
            "the name moved:\n{short:?}\n{long:?}"
        );
        assert_eq!(
            column(&short, "claude-opus-5"),
            column(&long, "claude-opus-5"),
            "the model moved:\n{short:?}\n{long:?}"
        );
    }

    #[test]
    fn the_name_is_against_the_left_edge() {
        let line = row("⣠⣾⠀⠀⠀");
        assert!(line.trim_start().starts_with("axum/main/alpha"), "{line:?}");
    }

    #[test]
    fn a_middle_too_long_for_its_room_is_dropped_rather_than_shoving() {
        // The ends are what the row is for. A middle with nowhere to go goes nowhere.
        let line = row(&"x".repeat(200));
        assert_eq!(line.chars().count(), 70, "{line:?}");
        assert!(line.trim_start().starts_with("axum/main/alpha"), "{line:?}");
        assert!(line.trim_end().ends_with("claude-opus-5"), "{line:?}");
    }
}

/// Both ends are held clear, and nothing is allowed to print into anything else.
#[cfg(test)]
mod inset_tests {
    use super::*;

    fn row(width: u16, identity: &str) -> String {
        crewed(width, identity, 1)
    }

    fn crewed(width: u16, identity: &str, crew: usize) -> String {
        let data = FooterData {
            input_tokens: 12_500,
            output_tokens: 900,
            context_percent: Some(6.2),
            context_window: 200_000,
            identity: identity.into(),
            model: "claude-opus-5".into(),
            crew,
            own: true,
            name_hover: false,
            model_hover: false,
        };
        render(&data, &[Span::raw("waiting")], width)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn both_ends_are_held_clear() {
        let pad = usize::from(crate::metric::footer_pad());
        let line = row(80, "axum/main/alpha");
        let head: String = line.chars().take(pad).collect();
        let tail: String = line.chars().skip(line.chars().count() - pad).collect();
        assert!(head.trim().is_empty(), "the left end: {line:?}");
        assert!(tail.trim().is_empty(), "the right end: {line:?}");
        assert_eq!(line.chars().count(), 80, "and the row is still the width");
    }

    #[test]
    fn the_middle_never_prints_into_the_name() {
        // What a centred middle does once the row is inset and nobody checks the column to its right.
        for width in 30..90u16 {
            let line = row(width, "axum/main/alpha");
            assert!(
                !line.contains("kaxum") && !line.contains("%axum"),
                "width {width}: {line:?}"
            );
            assert_eq!(line.chars().count(), usize::from(width), "width {width}");
        }
    }
}
/// And the display lands on the exact middle of the screen, not just of the space it was given.
#[cfg(test)]
mod middle_tests {
    use super::*;

    #[test]
    fn the_display_sits_on_the_screens_own_middle() {
        // The two ends are pinned to the edges, so anything off-centre between them is visible.
        for screen in 60..160u16 {
            let cells = crate::beacon::fitted(screen);
            let data = FooterData {
                identity: "axum/main/alpha".into(),
                model: "claude-opus-5".into(),
                ..FooterData::default()
            };
            let marks = vec![Span::raw("#".repeat(cells))];
            let line: String = render(&data, &marks, screen)[0]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            let at = line.find('#').map(|byte| line[..byte].chars().count());
            let Some(at) = at else {
                panic!("width {screen}: the display was dropped from {line:?}");
            };
            // Its own middle against the screen's: equal space either side, to the column.
            let after = usize::from(screen) - at - cells;
            assert_eq!(
                at, after,
                "width {screen}: {at} columns before it and {after} after"
            );
        }
    }
}
