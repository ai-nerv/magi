//! Asking the person anything, from where the tool is: the general form of [`crate::approve`],
//! carrying whatever the tool wanted to ask. Nobody attached means `None`, never an answer.

use magi_proto::tooling::Ask;

pub trait Asks: Send + Sync {
    /// Ask, and block until it is answered. `None` when nobody answered, which is not a refusal.
    fn ask(&self, tool: &str, ask: &Ask) -> Option<String>;
}

/// An asker nobody is behind, which answers nothing.
pub struct Unanswered;

impl Asks for Unanswered {
    fn ask(&self, _tool: &str, _ask: &Ask) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::tooling::Answer;

    #[test]
    fn nobody_attached_answers_nothing_rather_than_choosing() {
        let ask = Ask {
            question: "run it?".to_owned(),
            options: vec![Answer {
                id: "yes".to_owned(),
                label: "Yes".to_owned(),
                about: String::new(),
            }],
            detail: Vec::new(),
        };
        assert_eq!(Unanswered.ask("bash", &ask), None);
    }
}
