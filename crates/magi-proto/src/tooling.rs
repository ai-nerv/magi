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

/// The outcome of a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Text the model sees.
    pub output: String,
    /// Whether the tool failed.
    pub is_error: bool,
    /// What the *person* sees, when a tool said more than the text.
    ///
    /// The second of the two faces — see [`crate::tooling`]. `None` is what every tool produced
    /// before casper existed and what most still produce: draw `output` as plain text.
    ///
    /// Optional on the wire as well as in the type, so a journal written by a build that had
    /// never heard of it still loads. A transcript is the record of what happened, and a field
    /// added later must not make yesterday's session unreadable.
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
    /// Rows the tool is asking for, and will fill itself.
    ///
    /// The general form of [`Ask`]. A question has a shape magi chose; a surface has whatever
    /// shape its tenant draws, and magi cannot tell a permission prompt from a file picker from a
    /// game — which is the point. The list of things that can appear there is not a list anybody
    /// has to extend.
    ///
    /// magi owns *how much* room there is, because only magi knows what else is on the screen. It
    /// reserves, clips to the reservation, forwards input while the surface holds it, and blits
    /// back what comes out without reading it.
    Surface(Surface),
}

/// Rows a tool has asked for, and what to open to fill them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    /// How many rows it wants.
    ///
    /// A request, not a grant: magi gives it this many or fewer, and says which in the first
    /// frame. A tenant that drew past what it was given would run over whatever is below it.
    pub rows: u16,
    /// What this surface is for, in one line, for a harness that cannot draw it.
    ///
    /// `magi -p` has no screen and no person, and a run that silently drew nothing would look
    /// like a hang. It says this instead and declines.
    pub about: String,
    /// Milliseconds between ticks, for a surface that moves on its own.
    ///
    /// `None` for one that only answers input — a picker redraws when a key arrives and at no
    /// other time, and ticking it would be a wakeup a hundred times a second to draw the same
    /// rows. Something animating asks for a tick and gets one whether or not anybody is typing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u16>,
}

/// A run of text with one meaning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    /// What this text is.
    #[serde(default)]
    pub role: Role,
    /// The text itself.
    pub text: String,
    /// A colour chosen outright, overriding the role.
    ///
    /// **The exception to "a tool never chooses a colour", and it is narrow.** Roles exist so a
    /// `patch` and a highlighted `cat` agree with the rest of the screen: they are *output*, they
    /// are read alongside everything else, and a tool picking its own green would be a second
    /// palette to keep in step. A [`Shown::Surface`] is not that. It is a picture in rows nothing
    /// else is drawn in, and a dinosaur is brown whatever anybody's theme says — asking for
    /// `added` there would mean "green, because I want green", which is a role lying about itself.
    ///
    /// So: surfaces may use this, tool output should not. `None` everywhere else, and the role
    /// answers — which is what every tool that is not a picture keeps doing.
    ///
    /// Needs a terminal that speaks 24-bit colour. One that does not will approximate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb: Option<[u8; 3]>,
    /// A background chosen outright, for the same narrow reason as [`Span::rgb`].
    ///
    /// What makes a run of text read as *inverted* rather than merely coloured, which is how a
    /// picture says "this is held down right now" without a second row to say it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<[u8; 3]>,
}

impl Span {
    /// A span of `text` in `role`.
    #[must_use]
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            rgb: None,
            bg: None,
        }
    }

    /// A span of `text` in a colour of its own.
    ///
    /// For a surface drawing a picture. See [`Span::rgb`] for why that is the one place a chosen
    /// colour belongs.
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
        assert!(
            !wire.contains("colour") && !wire.contains("color"),
            "{wire}"
        );
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
        assert!(
            !wire.contains(r#""said""#),
            "an unfinished call said: {wire}"
        );
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

/// What a key did.
///
/// **A terminal only says this when it speaks the Kitty keyboard protocol.** Without it there is
/// one indistinguishable press per repeat and no word at all when a key comes back up, so nothing
/// can tell "tapped" from "still holding" — which is the whole difference between a hop and a jump
/// in anything reading the keyboard as a control rather than as text.
///
/// [`Self::Down`] is the default, and is what every key looks like on a terminal that cannot say
/// more. A tenant that only reads `Down` behaves the same either way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Held {
    /// It went down.
    #[default]
    Down,
    /// It is still down, and the terminal is repeating it.
    Repeat,
    /// It came back up.
    Up,
}

/// What the pointer did.
///
/// The wheel is here rather than beside it because a surface that scrolls and a surface that is
/// clicked are the same surface, and a tenant reading one field decides what it cares about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pointed {
    /// A button went down.
    #[default]
    Press,
    /// The pointer moved with a button held.
    Drag,
    /// A button came back up.
    Release,
    /// The pointer moved with nothing held.
    Moved,
    /// The wheel went up.
    ScrollUp,
    /// The wheel went down.
    ScrollDown,
}

/// Which button.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    /// The one everything uses.
    #[default]
    Left,
    /// The middle button, which on most mice is the wheel.
    Middle,
    /// The right button.
    Right,
}

/// What magi sends a surface while it holds its rows.
///
/// Frames rather than calls: a surface redraws per keystroke, so the spawn lives for the length of
/// the reservation instead of one exec per event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "to")]
pub enum ToSurface {
    /// The room it actually got, and what the call was given.
    ///
    /// The arguments travel with it so a surface opens knowing what it is about — a permission
    /// needs the command, a picker needs the list — rather than being told on some later frame.
    Open {
        /// Rows granted, which may be fewer than were asked for.
        rows: u16,
        /// Columns granted.
        cols: u16,
        /// Whether this terminal reports key repeats and releases.
        ///
        /// `false` on a terminal without the Kitty keyboard protocol, where every key arrives as
        /// a bare press. A tenant that would wait for a release is told there will never be one.
        #[serde(default)]
        holds: bool,
        /// The call's arguments.
        #[serde(default)]
        args: serde_json::Value,
    },
    /// A key the person pressed while this surface held the rows.
    ///
    /// Named, not a scancode: a tenant should not have to know how this terminal encodes a
    /// keypress, and magi has already decoded one to get here.
    Key {
        /// `j`, `enter`, `esc`, `ctrl+c`.
        key: String,
        /// Whether it went down, repeated, or came back up.
        #[serde(default)]
        state: Held,
    },
    /// The pointer, somewhere over the rows this surface holds.
    ///
    /// **In the surface's own coordinates.** Row 0, column 0 is its top-left cell, and nothing
    /// landing outside the reservation is forwarded at all. magi never says where those rows are
    /// on screen: they move whenever the prompt grows a line, and a tenant that had been told
    /// would be one magi could no longer place freely.
    ///
    /// This is what makes the rows a *screen* rather than somewhere keys are echoed. A picker can
    /// be clicked, a diff can be scrolled, and neither had to be given the keyboard first.
    Mouse {
        /// What it did.
        kind: Pointed,
        /// Which button, for the things a button does. Absent for motion and the wheel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<Button>,
        /// Rows down from the surface's own first row.
        row: u16,
        /// Columns across from the surface's own first column.
        col: u16,
    },
    /// The room changed under it, because the window did.
    Resize {
        /// Rows now.
        rows: u16,
        /// Columns now.
        cols: u16,
        /// Whether the keyboard reports holds, as currently known.
        ///
        /// Carried here as well as at open because it is *learned*: nothing proves the protocol is
        /// live until a repeat or a release arrives, which may be after the surface opened. A
        /// tenant told only at open would offer the lesser control for the rest of its life.
        #[serde(default)]
        holds: bool,
    },
    /// Time passed, for a surface that asked for it.
    ///
    /// Only sent to one that named a [`Surface::tick`]. A game needs the world to move while
    /// nobody is pressing anything, and a picker does not.
    Tick,
    /// The reservation is over and nothing more will be read.
    ///
    /// Sent when the turn is cancelled or the session ends, so a tenant holding state can put it
    /// down rather than being killed mid-write.
    Close,
}

/// What a surface sends back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "from")]
pub enum FromSurface {
    /// What to put in the rows, in the same roles everything else is painted in.
    ///
    /// Clipped to the reservation, never grown by it: a tenant that sent more rows than it was
    /// given would run over whatever is below it, and magi is the only one that knows what that
    /// is.
    Draw {
        /// Each row, as the spans it is made of.
        lines: Vec<Vec<Span>>,
    },
    /// The surface is finished, and this is what the person chose.
    ///
    /// An *id*, never a decision. A surface that returned "allowed" would be a sibling granting
    /// itself a permission, which is what the ledger exists to prevent — magi maps this id onto
    /// its own scopes and applies it.
    Done {
        /// The id of whatever was chosen, as the tool named it.
        answered: String,
    },
}

/// Rows a tool asks for, and what may cross while it holds them.
#[cfg(test)]
mod surfacing {
    use super::*;

    #[test]
    fn a_surface_asks_for_room_and_says_what_it_is_for() {
        // The second half is not decoration: `magi -p` has no screen, and a run that silently
        // drew nothing would look like a hang rather than like something declining.
        let asked = Shown::Surface(Surface {
            rows: 5,
            about: "a permission for `rm -rf build`".to_owned(),
            tick: None,
        });
        let wire = serde_json::to_string(&asked).expect("encodes");
        assert!(wire.contains(r#""shown":"surface""#), "{wire}");
        assert_eq!(
            serde_json::from_str::<Shown>(&wire).expect("decodes"),
            asked
        );
    }

    #[test]
    fn a_surface_can_say_what_was_chosen_and_not_what_it_means() {
        // The whole trust boundary in one assertion. There is no field here a tenant could set to
        // "allowed", "always" or any other scope: it returns the id it drew, and magi decides.
        let done = FromSurface::Done {
            answered: "once".to_owned(),
        };
        let wire = serde_json::to_string(&done).expect("encodes");
        for granting in ["allow", "grant", "scope", "permit", "decision"] {
            assert!(!wire.contains(granting), "{granting} crossed: {wire}");
        }
    }

    #[test]
    fn what_magi_sends_and_what_comes_back_are_different_types() {
        // One enum for both directions would let a tenant send `Key` and magi send `Draw`, and
        // the first thing either did with the other's frame would be to ask what it was.
        let open = serde_json::to_string(&ToSurface::Open {
            rows: 5,
            cols: 92,
            holds: true,
            args: serde_json::Value::Null,
        })
        .expect("encodes");
        assert!(
            serde_json::from_str::<FromSurface>(&open).is_err(),
            "{open}"
        );
    }

    #[test]
    fn a_key_crosses_by_name_rather_than_by_scancode() {
        // magi has already decoded a keypress to get here. Handing on the bytes would make every
        // tenant learn this terminal's encoding to read an `enter`.
        let wire = serde_json::to_string(&ToSurface::Key {
            key: "ctrl+c".to_owned(),
            state: Held::Down,
        })
        .expect("encodes");
        assert!(wire.contains(r#""key":"ctrl+c""#), "{wire}");
    }

    #[test]
    fn a_terminal_that_says_nothing_about_holding_says_down() {
        // Most of them, and every one before the Kitty protocol. A tenant that reads only `down`
        // behaves identically whether or not the terminal can say more.
        let plain: ToSurface =
            serde_json::from_str(r#"{"to":"key","key":"space"}"#).expect("decodes");
        assert_eq!(
            plain,
            ToSurface::Key {
                key: "space".to_owned(),
                state: Held::Down,
            }
        );
    }

    #[test]
    fn a_held_key_and_a_released_one_are_told_apart() {
        // The whole point of asking for event types: without them there is one indistinguishable
        // press per repeat and no word at all when a key comes back up.
        for (wire, state) in [
            (
                r#"{"to":"key","key":"space","state":"repeat"}"#,
                Held::Repeat,
            ),
            (r#"{"to":"key","key":"space","state":"up"}"#, Held::Up),
        ] {
            let read: ToSurface = serde_json::from_str(wire).expect("decodes");
            assert_eq!(
                read,
                ToSurface::Key {
                    key: "space".to_owned(),
                    state,
                }
            );
        }
    }

    #[test]
    fn a_click_crosses_in_the_surface_own_coordinates() {
        // Row zero is the tenant's first row, not the screen's. magi never says where the
        // reservation is, so a surface that had been told its own y would be one magi could no
        // longer move when the prompt grew a line.
        let wire = serde_json::to_string(&ToSurface::Mouse {
            kind: Pointed::Press,
            button: Some(Button::Left),
            row: 2,
            col: 11,
        })
        .expect("encodes");
        assert!(wire.contains(r#""row":2"#), "{wire}");
        assert!(wire.contains(r#""kind":"press""#), "{wire}");
        assert_eq!(
            serde_json::from_str::<ToSurface>(&wire).expect("decodes"),
            ToSurface::Mouse {
                kind: Pointed::Press,
                button: Some(Button::Left),
                row: 2,
                col: 11,
            }
        );
    }

    #[test]
    fn the_wheel_and_the_pointer_carry_no_button() {
        // There is none. A default `left` on the wire would have a tenant reading a scroll as a
        // click somebody never made.
        let wire = serde_json::to_string(&ToSurface::Mouse {
            kind: Pointed::ScrollDown,
            button: None,
            row: 0,
            col: 0,
        })
        .expect("encodes");
        assert!(!wire.contains("button"), "{wire}");
    }
}
