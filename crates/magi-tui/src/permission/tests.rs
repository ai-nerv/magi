use super::*;

fn said(rendered: &Rendered) -> Vec<String> {
    rendered
        .rows
        .iter()
        .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn deciding() -> Judging {
    Judging {
        mode: Mode::Auto,
        model: Some("decisions/typesafe/jev-1.13".into()),
        kind: Kind::Decides,
        judged: 9,
        refused: 2,
        in_a_row: 1,
        denied: vec!["run `sudo`".into()],
        always_asked: vec!["run `git`".into()],
        ..Judging::default()
    }
}

#[test]
fn what_may_be_changed_is_what_carries_the_arrows() {
    let card = view(&deciding(), 60);
    let steppable: Vec<&String> = card.picks.iter().flatten().collect();
    assert_eq!(steppable, ["mode", "band"], "and nothing else is offered");
    for (row, pick) in said(&card).iter().zip(&card.picks) {
        assert_eq!(row.contains('◂'), pick.is_some(), "{row}");
    }
}

#[test]
fn the_mode_says_what_it_does_rather_than_only_its_name() {
    for mode in Mode::ALL {
        let card = view(&Judging { mode, ..deciding() }, 60);
        let rows = said(&card);
        assert_eq!(rows[1], does(mode));
        assert!(rows[3].contains(mode.name()), "{:?}", rows[3]);
    }
}

#[test]
fn the_band_is_drawn_where_it_sits_and_says_what_each_part_of_it_does() {
    let rows = said(&view(
        &Judging {
            unsure: (0.2, 0.8),
            ..deciding()
        },
        60,
    ));
    let slider = rows.iter().find(|row| row.contains('░')).expect("a band");
    // A fifth refused, a fifth allowed, and the three fifths between them the person's.
    assert_eq!(slider.matches('░').count(), SLIDER * 3 / 5);
    assert!(rows.iter().any(|r| r.contains("refused <0.20")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("allowed >0.80")), "{rows:?}");
    // Moved, it is drawn where it was moved to.
    let narrow = said(&view(
        &Judging {
            unsure: (0.45, 0.55),
            ..deciding()
        },
        60,
    ));
    let slider = narrow.iter().find(|row| row.contains('░')).expect("a band");
    assert!(slider.matches('░').count() < SLIDER / 4, "{slider}");
}

#[test]
fn a_model_that_writes_is_told_apart_from_one_that_decides() {
    let writes = said(&view(
        &Judging {
            kind: Kind::Writes,
            model: Some("openrouter/deepseek/deepseek-v4-flash-0731".into()),
            ..deciding()
        },
        60,
    ));
    assert!(
        writes.iter().any(|r| r.contains("writes — asked in words")),
        "{writes:?}"
    );
    // No number from it, so no band to draw, and the card says why rather than showing one.
    assert!(!writes.iter().any(|r| r.contains('░')));
    assert!(
        writes
            .iter()
            .any(|r| r.contains("only a model that decides")),
        "{writes:?}"
    );
    assert_eq!(
        view(
            &Judging {
                kind: Kind::Writes,
                ..deciding()
            },
            60
        )
        .picks
        .iter()
        .flatten()
        .count(),
        1,
        "the band cannot be stepped when there is none"
    );
    // Nobody at all says how to get one.
    let none = said(&view(&Judging::default(), 60));
    assert!(
        none.iter().any(|r| r.contains("magi.helpers.safety")),
        "{none:?}"
    );
}

#[test]
fn the_rules_that_hold_in_every_mode_are_named_where_a_person_set_them() {
    let rows = said(&view(&deciding(), 60));
    assert!(rows.iter().any(|r| r.contains("magi.deny")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("run `sudo`")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("run `git`")), "{rows:?}");
    // With none set, what to set them with.
    let bare = said(&view(
        &Judging {
            denied: Vec::new(),
            always_asked: Vec::new(),
            ..deciding()
        },
        60,
    ));
    assert!(bare.iter().any(|r| r.contains("none set")), "{bare:?}");
    // And the one thing none of this can turn off.
    assert!(rows.iter().any(|r| r.contains("kernel jail")), "{rows:?}");
}

#[test]
fn what_it_has_done_this_session_is_shown_once_it_has_done_anything() {
    let rows = said(&view(&deciding(), 60));
    assert!(
        rows.iter().any(|r| r.contains("9, refused 2 of them")),
        "{rows:?}"
    );
    let fresh = said(&view(
        &Judging {
            judged: 0,
            ..deciding()
        },
        60,
    ));
    assert!(!fresh.iter().any(|r| r.starts_with("Judged")), "{fresh:?}");
}
