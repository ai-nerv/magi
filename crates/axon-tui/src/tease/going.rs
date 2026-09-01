//! Where it goes next, and how long each moment lasts.

use super::*;

/// It chooses the line it can make the smallest edit into.
#[cfg(test)]
mod picking_tests {
    use super::*;

    fn pool() -> Vec<String> {
        [
            "the scaffolding is temporary",
            "the scaffolding is the building",
            "we're shipping and watching the graphs",
        ]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
    }

    #[test]
    fn it_prefers_a_line_it_can_edit_into() {
        let none = VecDeque::new();
        // The whole point of the engine. Picking at random would retype the line most of the
        // time, and the middle edit -- walk, mark, cut, type -- would almost never be seen.
        let lines = pool();
        for _ in 0..8 {
            assert_eq!(
                pick(&lines, "the scaffolding is temporary", &none),
                "the scaffolding is the building"
            );
        }
    }

    #[test]
    fn it_still_answers_when_nothing_is_close() {
        // A pool of unrelated lines is not an error; it just means more retyping.
        let lines = vec!["alpha beta".to_owned()];
        assert_eq!(pick(&lines, "gamma delta", &VecDeque::new()), "alpha beta");
    }

    #[test]
    fn it_never_offers_the_line_already_up() {
        let lines = pool();
        let none = VecDeque::new();
        for _ in 0..8 {
            assert_ne!(
                pick(&lines, "the scaffolding is temporary", &none),
                "the scaffolding is temporary"
            );
        }
    }

    #[test]
    fn kinship_counts_both_ends() {
        assert_eq!(kinship(&["a", "b", "c"], &["a", "x", "c"]), 2);
        assert_eq!(kinship(&["a", "b"], &["x", "y"]), 0);
        assert_eq!(kinship(&["a", "b"], &["a", "b"]), 2, "and does not double");
    }
}

/// It moves on, and what it changes is in the middle.
#[cfg(test)]
mod wandering_tests {
    use super::*;

    fn pool() -> Vec<String> {
        [
            "this is a temporary fix that will outlive us all",
            "this is a permanent fix that will outlive us all",
            "this is a clever fix that will outlive us all",
            "the roadmap is a list of wishes, sorted by hope",
            "the roadmap is a list of bugs, sorted by hope",
        ]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
    }

    /// The lines it walks through, following its own choices.
    fn walked(steps: usize) -> Vec<String> {
        let lines = pool();
        let mut shown = lines[0].clone();
        let mut seen: VecDeque<String> = VecDeque::from([shown.clone()]);
        let mut out = vec![shown.clone()];
        for _ in 0..steps {
            let next = pick(&lines, &shown, &seen).to_owned();
            let mut tease = Tease::new(&shown);
            for act in perform(&shown, &next, 0) {
                tease.play(&act);
            }
            shown = tease.shown().to_owned();
            seen.push_back(shown.clone());
            while seen.len() > RECALLED {
                seen.pop_front();
            }
            out.push(shown.clone());
        }
        out
    }

    #[test]
    fn it_does_not_get_stuck_between_two_lines() {
        // The bug this exists for. Picking the closest line and nothing else means the closest
        // line to *that* is the one it came from, so a family of two points at itself and the
        // box swaps between them until somebody types.
        let walk = walked(6);
        let mut distinct = walk.clone();
        distinct.sort();
        distinct.dedup();
        assert!(
            distinct.len() >= 4,
            "it only ever said {} different things: {walk:#?}",
            distinct.len()
        );
    }

    #[test]
    fn every_line_it_lands_on_is_one_from_the_pool() {
        // Whatever route it takes, the acts have to add up to a line somebody wrote.
        let lines = pool();
        for said in walked(6) {
            assert!(lines.contains(&said), "{said:?} is not in the pool");
        }
    }

    #[test]
    fn what_changes_has_words_on_both_sides_of_it() {
        // "a word in the middle", which is the whole ask. A family whose lines differ only at
        // the end can only ever have its tail retyped, and the walk has nothing to walk past.
        let lines = pool();
        for from in &lines {
            let to = pick(&lines, from, &VecDeque::new());
            let (cut, _) = difference(&words(from), &words(to));
            let total = words(from).len();
            assert!(cut.start > 0, "{from:?} into {to:?} changes the first word");
            assert!(
                cut.end < total,
                "{from:?} into {to:?} changes the last word"
            );
        }
    }
}

/// A duration holds what the act did, rather than delaying it.
#[cfg(test)]
mod timing_tests {
    use super::*;

    /// The script for a one-word change.
    fn script() -> VecDeque<Act> {
        perform(
            "this is a temporary fix that will outlive us all",
            "this is a clever fix that will outlive us all",
            0,
        )
    }

    #[test]
    fn the_long_pause_is_on_the_selection_and_not_before_it() {
        // The bug this exists for. An act's duration used to be the wait *before* it played, so
        // the three-fold hold went on the pause with the cursor sitting there doing nothing, and
        // the inverted words appeared and were gone again in one step.
        let script = script();
        let mark = script
            .iter()
            .find_map(|act| match act {
                Act::Mark { over, .. } => Some(*over),
                _ => None,
            })
            .expect("it marks");
        let typing = script
            .iter()
            .find_map(|act| match act {
                Act::Put { over, .. } => Some(*over),
                _ => None,
            })
            .expect("it types");
        assert!(
            mark > typing * 10,
            "the selection is held {mark:?} against {typing:?} a keystroke"
        );
    }

    #[test]
    fn the_mark_is_still_up_while_it_is_being_held() {
        // Which is the whole of what "held" means: play the mark, and the inversion is what is
        // on screen until the next act takes it away.
        let mut tease = Tease::new("this is a temporary fix that will outlive us all");
        for act in script() {
            tease.play(&act);
            if matches!(act, Act::Mark { .. }) {
                assert!(tease.marked.is_some(), "nothing is inverted");
                return;
            }
        }
        panic!("the script never marked anything");
    }

    #[test]
    fn a_performance_ends_with_the_ghost_back_in_normal_mode() {
        // A bar sitting still for thirty seconds looks like a prompt waiting for you rather
        // than a box that has stopped.
        let mut tease = Tease::new("this is a temporary fix that will outlive us all");
        for act in script() {
            tease.play(&act);
        }
        assert!(tease.block, "it rested with a bar cursor");
    }
}
