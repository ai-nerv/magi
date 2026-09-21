//! What a list does: filtering, taking a row, and taking one away.
//!
//! Split out under THE RULE; the list itself is next door.

use super::*;

#[cfg(test)]
mod removing {
    use super::*;

    fn rows() -> Vec<Choice> {
        ["one", "two"]
            .iter()
            .map(|value| Choice {
                value: (*value).to_owned(),
                detail: String::new(),
                ready: true,
            })
            .collect()
    }

    #[test]
    fn a_list_that_was_not_told_it_may_remove_anything_does_not_ask() {
        let mut list = Picker::new("pick", rows(), None);
        assert!(!list.ask(), "asked about a list with nothing to take away");
        assert!(list.asking().is_none());
    }

    #[test]
    fn the_question_names_the_row_and_opens_on_no() {
        let mut list = Picker::new("pick", rows(), None).askable("Remove {}?");
        assert!(list.ask());
        let asking = list.asking().expect("a question");
        assert_eq!(asking.question, "Remove one?");
        assert!(!asking.yes, "a stray Enter has to answer the safe way");
    }

    #[test]
    fn nothing_goes_until_the_answer_is_yes() {
        let mut list = Picker::new("pick", rows(), None).askable("Remove {}?");
        list.ask();
        assert_eq!(list.answer(false), None, "no takes the question down only");
        assert!(list.asking().is_none());

        list.ask();
        list.swap();
        assert!(list.asking().expect("a question").yes);
        assert_eq!(list.answer(true), Some("one".to_owned()));
        assert!(list.asking().is_none(), "and the question goes with it");
    }

    #[test]
    fn answering_when_nothing_was_asked_takes_nothing() {
        let mut list = Picker::new("pick", rows(), None).askable("Remove {}?");
        assert_eq!(list.answer(true), None);
    }
}
