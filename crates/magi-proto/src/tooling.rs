//! The magi↔casper contract: what a tool is, and the two faces of what it produced.
//!
//! casper owns the tools. magi asks it what exists, hands it a call, and draws what comes back.
//!
//! # Two faces
//!
//! A tool result is read by two different readers and they do not want the same thing. The model
//! wants text it can reason about; the person wants it drawn the way the rest of the screen is
//! drawn. So a [`Ran`] carries [`Ran::said`] for the model and [`Ran::shown`] for the screen, and
//! either may be absent: a `bash` has no view, a permission question has no result yet.
//!
//! # Paint carries meaning, not colour
//!
//! A [`Span`] names a [`Role`] and never a colour. magi resolves the role against its own palette,
//! which is what makes a `patch` and a highlighted `cat` agree: both emit `added` or `keyword`,
//! and one palette paints them. A tool that chose colours would be a second palette to keep in
//! step, and it would be wrong on the first theme somebody set.

use serde::{Deserialize, Serialize};

/// One tool, as casper describes it.
///
/// The same three things magi's own tools carry — a name, a description and a schema — plus what
/// the tool would need permission for. casper describes; magi decides. A sibling that could grant
/// itself a permission would make the ledger a suggestion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// The name the model calls it by.
    pub name: String,
    /// What it does, in the model's terms.
    pub description: String,
    /// JSON Schema for its arguments.
    pub parameters: serde_json::Value,
    /// The permission verb this tool acts under, if it needs one.
    ///
    /// `read`, `write`, `run`, `reach` — magi's own vocabulary, because magi is what answers.
    /// `None` for a tool that touches nothing a person would want a say over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

/// One call, on its way to casper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// Which tool.
    pub tool: String,
    /// Its arguments, as the model gave them.
    pub args: serde_json::Value,
    /// Where the session is rooted, so a relative path means what the person means.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    /// An answer to the question the last [`Ask`] posed, when this call is resuming one.
    ///
    /// The id of the chosen option. A tool that asked nothing ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<String>,
}

/// What a tool produced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ran {
    /// What the model reads.
    ///
    /// Empty for a tool that has not finished — one waiting on an answer has produced nothing
    /// yet, and sending the model an empty result would end the call it is still in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub said: String,
    /// Whether it failed.
    ///
    /// A tool that ran and reported a problem is still a result: the model needs to read what
    /// went wrong in order to do something about it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    /// What the person sees, when it is more than the text.
    ///
    /// `None` means "draw `said` as plain text", which is what every tool does before anybody
    /// writes it a view. Nothing has to be ported for casper to be useful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<Shown>,
}

/// What magi draws for this result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "shown")]
pub enum Shown {
    /// Painted lines, in roles magi resolves against its palette.
    Painted {
        /// Each line, as the spans it is made of.
        lines: Vec<Vec<Span>>,
    },
    /// A question for the person, and the answers they may give.
    ///
    /// The tool has not finished. magi draws this, sends the chosen id back as
    /// [`Call::answered`], and the call resumes — which is the same mechanism a permission, a
    /// file picker and a confirmation all need.
    Ask(Ask),
}

/// A run of text with one meaning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    /// What this text is.
    #[serde(default)]
    pub role: Role,
    /// The text itself.
    pub text: String,
}

impl Span {
    /// A span of `text` in `role`.
    #[must_use]
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
        }
    }
}

/// What a span of text *is*, which magi turns into a colour.
///
/// Closed on purpose. An open vocabulary is a second palette: a tool naming its own role would be
/// asking magi to invent a colour for it, and the answer would differ per tool. Four families,
/// because that is what the tools actually produce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    // Prose.
    /// Ordinary text.
    #[default]
    Text,
    /// Present but secondary.
    Muted,
    /// Present and nearly out of the way.
    Dim,
    /// A heading, or the name of the thing below it.
    Title,
    /// A path, a filename, a location.
    Path,

    // Outcome.
    /// It worked.
    Ok,
    /// It worked, and something is worth knowing.
    Warn,
    /// It did not work.
    Error,

    // Change.
    /// A line a patch adds.
    Added,
    /// A line a patch removes.
    Removed,
    /// The `@@` and `+++` rows, which say *where* rather than what.
    Marker,
    /// A line a patch leaves alone.
    Context,

    // Code.
    /// A language keyword.
    Keyword,
    /// A string literal.
    String,
    /// A numeric literal.
    Number,
    /// A comment.
    Comment,
    /// A type name.
    Type,
    /// A function name.
    Func,
}

/// A question a tool is putting to the person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    /// What is being asked, in one line.
    pub question: String,
    /// What may be answered.
    ///
    /// Never empty: a question with no answers is a message, and a message is `said`.
    pub options: Vec<Answer>,
    /// More about what is being asked, for the rows under the question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<Vec<Span>>,
}

/// One answer to an [`Ask`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// What comes back as [`Call::answered`].
    pub id: String,
    /// What the row says.
    pub label: String,
    /// A second line, when the label alone does not say what it means.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_with_nothing_to_show_is_the_text_and_no_more() {
        // What every tool does before anybody writes it a view. The absent fields do not travel:
        // a `bash` result should not carry three nulls describing what it is not.
        let ran = Ran {
            said: "a\nb".to_owned(),
            ..Ran::default()
        };
        let wire = serde_json::to_string(&ran).expect("encodes");
        assert_eq!(wire, r#"{"said":"a\nb"}"#);
        assert_eq!(serde_json::from_str::<Ran>(&wire).expect("decodes"), ran);
    }

    #[test]
    fn a_painted_result_keeps_its_roles_and_never_names_a_colour() {
        // The whole point. A tool that sent a colour would be a second palette, and it would
        // disagree with the first the moment somebody set a theme.
        let ran = Ran {
            said: "-was\n+now".to_owned(),
            shown: Some(Shown::Painted {
                lines: vec![
                    vec![Span::new(Role::Removed, "-was")],
                    vec![Span::new(Role::Added, "+now")],
                ],
            }),
            ..Ran::default()
        };
        let wire = serde_json::to_string(&ran).expect("encodes");
        assert!(wire.contains(r#""role":"removed""#), "{wire}");
        assert!(!wire.contains("colour") && !wire.contains("color"), "{wire}");
        assert_eq!(serde_json::from_str::<Ran>(&wire).expect("decodes"), ran);
    }

    #[test]
    fn a_question_carries_no_result_because_the_tool_has_not_finished() {
        // The distinction `said` and `shown` exist for. Sending the model an empty result here
        // would end a call that is still waiting on a person.
        let ran = Ran {
            shown: Some(Shown::Ask(Ask {
                question: "run `rm -rf build`?".to_owned(),
                options: vec![
                    Answer {
                        id: "once".to_owned(),
                        label: "Allow once".to_owned(),
                        about: String::new(),
                    },
                    Answer {
                        id: "no".to_owned(),
                        label: "Deny".to_owned(),
                        about: "the model is told, and carries on".to_owned(),
                    },
                ],
                detail: Vec::new(),
            })),
            ..Ran::default()
        };
        assert!(ran.said.is_empty());
        let wire = serde_json::to_string(&ran).expect("encodes");
        assert!(!wire.contains(r#""said""#), "an unfinished call said: {wire}");
        assert_eq!(serde_json::from_str::<Ran>(&wire).expect("decodes"), ran);
    }

    #[test]
    fn the_two_kinds_of_view_are_told_apart_by_the_tag() {
        // A reader that could not tell a painted result from a question would draw a picker as
        // text, or wait for an answer to a diff.
        let painted = serde_json::to_string(&Shown::Painted { lines: Vec::new() }).expect("enc");
        assert!(painted.contains(r#""shown":"painted""#), "{painted}");
        let ask = serde_json::to_string(&Shown::Ask(Ask {
            question: "?".to_owned(),
            options: Vec::new(),
            detail: Vec::new(),
        }))
        .expect("enc");
        assert!(ask.contains(r#""shown":"ask""#), "{ask}");
    }

    #[test]
    fn a_card_never_grants_itself_anything() {
        // casper describes what a tool would do; magi decides whether it may. A card carries the
        // verb and no answer to it, so there is nothing here a sibling could set to "allowed".
        let card = Card {
            name: "bash".to_owned(),
            description: "Run a command.".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            needs: Some("run".to_owned()),
        };
        let wire = serde_json::to_string(&card).expect("encodes");
        assert!(!wire.contains("allow") && !wire.contains("grant"), "{wire}");
        assert_eq!(serde_json::from_str::<Card>(&wire).expect("decodes"), card);
    }

    #[test]
    fn a_role_that_is_not_given_is_ordinary_text() {
        // So an adapter that knows nothing about a line can still emit it.
        let span: Span = serde_json::from_str(r#"{"text":"hello"}"#).expect("decodes");
        assert_eq!(span.role, Role::Text);
    }

    #[test]
    fn every_role_round_trips_by_the_name_it_is_written_with() {
        // Both sides read this vocabulary from the same list or they do not agree at all, and a
        // renamed variant is a role that silently becomes `text` on the far side.
        for (role, name) in [
            (Role::Added, "added"),
            (Role::Removed, "removed"),
            (Role::Marker, "marker"),
            (Role::Context, "context"),
            (Role::Keyword, "keyword"),
            (Role::Comment, "comment"),
            (Role::Path, "path"),
            (Role::Error, "error"),
        ] {
            let wire = serde_json::to_string(&role).expect("encodes");
            assert_eq!(wire, format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<Role>(&wire).expect("decodes"), role);
        }
    }
}
