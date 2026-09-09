//! The magi↔casper contract: what a tool is, and the two faces of what it produced.
//!
//! A [`Ran`] carries [`Ran::said`] for the model and [`Ran::shown`] for the screen, and either may
//! be absent. A [`Span`] names a [`Role`] and never a colour; magi resolves the role against its
//! own palette.

use serde::{Deserialize, Serialize};

/// One tool, as casper describes it. casper describes and magi decides: a sibling that could grant
/// itself a permission would make the ledger a suggestion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    /// The permission verb this tool acts under — `read`, `write`, `run`, `reach`, magi's own
    /// vocabulary. `None` for a tool that touches nothing a person would want a say over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub tool: String,
    pub args: serde_json::Value,
    /// Where the session is rooted, so a relative path means what the person means.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    /// The id of the option chosen in answer to the last [`Ask`], when this call resumes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ran {
    /// What the model reads. Empty for a tool that has not finished: sending the model an
    /// empty result would end the call it is still in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub said: String,
    /// Whether it failed. A tool that ran and reported a problem is still a result.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<Shown>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
    /// What the person sees, when a tool said more than the text. Optional on the wire as well as
    /// in the type, so a journal written before it existed still loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<Shown>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "shown")]
pub enum Shown {
    Painted {
        lines: Vec<Vec<Span>>,
    },
    /// A question for the person. The tool has not finished: magi draws this, sends the chosen id
    /// back as [`Call::answered`], and the call resumes.
    Ask(Ask),
    /// Rows the tool is asking for, and will fill itself. magi owns how much room there is, and
    /// reserves, clips, forwards input and blits back what comes out without reading it.
    Surface(Surface),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    /// How many rows it wants. A request, not a grant: magi gives this many or fewer and says
    /// which in the first frame.
    pub rows: u16,
    pub about: String,
    /// Milliseconds between ticks, for a surface that moves on its own; `None` for one that only
    /// answers input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    #[serde(default)]
    pub role: Role,
    pub text: String,
    /// A colour chosen outright, overriding the role. For a [`Shown::Surface`] only: a picture is
    /// drawn in rows nothing else is, and `added` there would be a role lying about itself. Tool
    /// output keeps to roles. Needs a terminal that speaks 24-bit colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb: Option<[u8; 3]>,
    /// A background chosen outright, for the same narrow reason as [`Span::rgb`]. What makes a run
    /// of text read as inverted rather than merely coloured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<[u8; 3]>,
}

impl Span {
    #[must_use]
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            rgb: None,
            bg: None,
        }
    }

    #[must_use]
    pub fn painted(rgb: [u8; 3], text: impl Into<String>) -> Self {
        Self {
            role: Role::Text,
            text: text.into(),
            rgb: Some(rgb),
            bg: None,
        }
    }
}

/// What a span of text is, which magi turns into a colour. Closed: an open vocabulary would be a
/// second palette.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    // Prose.
    #[default]
    Text,
    Muted,
    Dim,
    Title,
    Path,

    // Outcome.
    Ok,
    Warn,
    Error,

    // Change.
    Added,
    Removed,
    Marker,
    Context,

    // Code.
    Keyword,
    String,
    Number,
    Comment,
    Type,
    Func,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub question: String,
    /// What may be answered. Never empty: a question with no answers is a message, and a message
    /// is `said`.
    pub options: Vec<Answer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<Vec<Span>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_with_nothing_to_show_is_the_text_and_no_more() {
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
        assert!(
            !wire.contains("colour") && !wire.contains("color"),
            "{wire}"
        );
        assert_eq!(serde_json::from_str::<Ran>(&wire).expect("decodes"), ran);
    }

    #[test]
    fn a_question_carries_no_result_because_the_tool_has_not_finished() {
        // Sending the model an empty result here would end a call still waiting on a person.
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
        assert!(
            !wire.contains(r#""said""#),
            "an unfinished call said: {wire}"
        );
        assert_eq!(serde_json::from_str::<Ran>(&wire).expect("decodes"), ran);
    }

    #[test]
    fn the_two_kinds_of_view_are_told_apart_by_the_tag() {
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
        let span: Span = serde_json::from_str(r#"{"text":"hello"}"#).expect("decodes");
        assert_eq!(span.role, Role::Text);
    }

    #[test]
    fn every_role_round_trips_by_the_name_it_is_written_with() {
        // Both sides read this vocabulary from the same list, and a renamed variant is a role that
        // silently becomes `text` on the far side.
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
