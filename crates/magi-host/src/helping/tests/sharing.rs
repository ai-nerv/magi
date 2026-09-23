//! Which jobs wait for the whole turn, and which may start while it is going.

use super::*;

fn job(role: &str) -> Job {
    Job {
        role: role.into(),
        fallback: "main".into(),
        ..Job::default()
    }
}

fn roles(named: &[(&str, &str)]) -> Helpers {
    Helpers {
        roles: named
            .iter()
            .map(|(role, model)| ((*role).to_owned(), (*model).to_owned()))
            .collect(),
        ..Helpers::default()
    }
}

#[test]
fn a_helper_on_the_turns_own_model_waits_for_the_turn() {
    // A notes job released between two rounds of a tool loop held the next round for
    // 49 seconds: the server answers one request at a time, and it was answering the job.
    let (mine, elsewhere) = sharing(vec![job("notes")], &roles(&[]), "ollama/big");
    assert_eq!(mine.len(), 1, "falls back to the main model, so it waits");
    assert!(elsewhere.is_empty());
}

#[test]
fn a_helper_named_to_the_main_model_waits_as_well() {
    let set = roles(&[("memory", MAIN)]);
    let (mine, elsewhere) = sharing(vec![job("notes")], &set, "ollama/big");
    assert_eq!((mine.len(), elsewhere.len()), (1, 0));
}

#[test]
fn one_on_another_model_goes_while_the_turn_is_going() {
    // Nothing is shared, so there is nothing to wait for, and memory is fresher for it.
    let set = roles(&[("memory", "openrouter/small")]);
    let (mine, elsewhere) = sharing(vec![job("notes")], &set, "ollama/big");
    assert_eq!((mine.len(), elsewhere.len()), (0, 1));
}

#[test]
fn a_mixed_batch_is_split_rather_than_held_whole() {
    let set = roles(&[("decision", "decisions/judge")]);
    let (mine, elsewhere) = sharing(
        vec![job("notes"), job("decision"), job("summary")],
        &set,
        "ollama/big",
    );
    let named = |jobs: &[Job]| jobs.iter().map(|j| j.role.clone()).collect::<Vec<_>>();
    assert_eq!(named(&mine), ["notes", "summary"]);
    assert_eq!(named(&elsewhere), ["decision"]);
}
