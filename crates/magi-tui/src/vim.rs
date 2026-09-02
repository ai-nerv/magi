//! Modes, motions and operators.
//!
//! The prompt is modal: it opens in normal mode and does not take text until you ask it to.
//! That is the whole point of the thing — a key press in normal mode is a command, and typing
//! into a buffer is one command among the rest rather than the default.
//!
//! There is no setting for it. A modal editor that can be switched off is two editors to keep
//! working, and the second one is the one nobody tests.
//!
//! What lives here is the *vocabulary*: which mode the prompt is in, and what a key means in
//! normal mode. Applying that to a buffer is [`crate::editor`]'s job, and deciding what a
//! non-editing command does — scrolling, submitting, searching — belongs to the caller, which
//! is the only thing that knows there is a transcript.
//!
//! **Operators and motions compose**, as they should: `d` `c` and `y` each wait for a motion
//! and act over the ground it covers, so `dw` `d$` `ct,` `yb` all work without being written
//! out one by one. That is the difference between vim bindings and vim.

/// Which mode the prompt is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Keys are commands. Where the prompt opens, and where `esc` returns it.
    #[default]
    Normal,
    /// Keys are text.
    Insert,
    /// Keys are a command line, and the prompt is holding your text for you.
    ///
    /// Its own mode rather than a colon typed into the buffer. `:` used to insert one and let
    /// the completion menu notice it, which meant a colon typed *in insert mode* — in the
    /// middle of a sentence, in a path, in a ratio — opened the command menu over the prompt.
    /// A command line is a different buffer, so this is a different mode.
    Command,
}

impl Mode {
    /// The three letters shown on the prompt box.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Normal => "NOR",
            Self::Insert => "INS",
            Self::Command => "CMD",
        }
    }

    /// Whether typing a printable character puts it in the buffer.
    #[must_use]
    pub fn is_insert(self) -> bool {
        matches!(self, Self::Insert | Self::Command)
    }
}

/// What a key means in normal mode.
///
/// Motions and edits are separated from everything else because the first two are the editor's
/// and the rest are the caller's: `Scroll` needs a transcript, `Submit` needs a daemon, and
/// neither is something a text buffer should know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deed {
    /// Move the cursor.
    Move(Motion),
    /// Change the buffer where it stands.
    Edit(Edit),
    /// Wait for a motion, then act over the ground it covers.
    Operate(Operator),
    /// Wait for the character `f`, `t`, `F`, `T` or `r` is about to be given.
    Await(Wants),
    /// Enter insert mode, having first done `Edit` — `a` moves right, `o` opens a line.
    Insert(Option<Edit>),
    /// Open the command line.
    Command,
    /// Move the transcript.
    Scroll(Toward),
    /// Start a search.
    Search,
    /// Go to the next or previous match.
    Match { forward: bool },
    /// Go back one change.
    Undo,
    /// Send what is in the buffer.
    Submit,
    /// Nothing is bound to this key.
    Unbound,
}

/// A cursor movement, and the ground an operator covers when given one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// `h`
    Left,
    /// `l`
    Right,
    /// `j`
    Down,
    /// `k`
    Up,
    /// `w`
    WordRight,
    /// `b`
    WordLeft,
    /// `e`
    WordEnd,
    /// `0`
    LineStart,
    /// `^`
    FirstWord,
    /// `$`
    LineEnd,
    /// `gg`
    First,
    /// `G`
    Last,
    /// `f`, `t`, `F`, `T`, with the character they were given.
    ToChar {
        /// What to look for.
        target: char,
        /// Forwards through the line, or backwards.
        forward: bool,
        /// Stop one short of it, which is what `t` and `T` do.
        short: bool,
    },
}

/// A change made where the cursor stands, with no motion to wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// `x`
    DeleteChar,
    /// `X`
    DeleteBack,
    /// `dd`
    DeleteLine,
    /// `yy`
    YankLine,
    /// `D`
    KillToEnd,
    /// `p`
    PasteAfter,
    /// `P`
    PasteBefore,
    /// `o`
    OpenBelow,
    /// `O`
    OpenAbove,
    /// `J`
    Join,
    /// `~`
    FlipCase,
    /// `r`, with the character it was given.
    Replace(char),
    /// The motion half of `A`, `I` and friends, which move without changing anything.
    Go(Motion),
}

/// An operator waiting for a motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `d`
    Delete,
    /// `c`
    Change,
    /// `y`
    Yank,
}

impl Operator {
    /// The key that doubles it into a whole-line command: `dd`, `cc`, `yy`.
    #[must_use]
    pub fn key(self) -> char {
        match self {
            Self::Delete => 'd',
            Self::Change => 'c',
            Self::Yank => 'y',
        }
    }
}

/// A key that needs one more before it means anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wants {
    /// `f` — forwards, onto it.
    Find,
    /// `t` — forwards, up to it.
    Till,
    /// `F` — backwards, onto it.
    FindBack,
    /// `T` — backwards, up to it.
    TillBack,
    /// `r` — replace what is under the cursor with it.
    Replace,
}

impl Wants {
    /// What this key does once it has the character it was waiting for.
    #[must_use]
    pub fn given(self, target: char) -> Deed {
        match self {
            Self::Find => Deed::Move(Motion::ToChar {
                target,
                forward: true,
                short: false,
            }),
            Self::Till => Deed::Move(Motion::ToChar {
                target,
                forward: true,
                short: true,
            }),
            Self::FindBack => Deed::Move(Motion::ToChar {
                target,
                forward: false,
                short: false,
            }),
            Self::TillBack => Deed::Move(Motion::ToChar {
                target,
                forward: false,
                short: true,
            }),
            Self::Replace => Deed::Edit(Edit::Replace(target)),
        }
    }
}

/// Where the transcript is being asked to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toward {
    /// `ctrl+u`
    HalfUp,
    /// `ctrl+d`
    HalfDown,
    /// `k` on a single-line prompt
    LineUp,
    /// `j` on a single-line prompt
    LineDown,
    /// `gg` on a single-line prompt
    Top,
    /// `G` on a single-line prompt
    Bottom,
}

/// A key in normal mode, given how tall the prompt is.
///
/// `tall` decides what `j`, `k`, `gg` and `G` mean, and it is the one genuinely awkward thing
/// here. A single-line prompt has nowhere to move up or down *to*, so they move the transcript,
/// which is what somebody reading back through output wants. A prompt with several lines has
/// somewhere to go, so they go there.
#[must_use]
pub fn deed(key: char, tall: bool) -> Deed {
    match key {
        'i' => Deed::Insert(None),
        'a' => Deed::Insert(Some(Edit::Go(Motion::Right))),
        'I' => Deed::Insert(Some(Edit::Go(Motion::FirstWord))),
        'A' => Deed::Insert(Some(Edit::Go(Motion::LineEnd))),
        'o' => Deed::Insert(Some(Edit::OpenBelow)),
        'O' => Deed::Insert(Some(Edit::OpenAbove)),
        // `C` and `S` are `c$` and `cc` under another name, and vim spells them both ways.
        'C' => Deed::Insert(Some(Edit::KillToEnd)),
        'S' => Deed::Insert(Some(Edit::DeleteLine)),

        'h' => Deed::Move(Motion::Left),
        'l' | ' ' => Deed::Move(Motion::Right),
        'j' if tall => Deed::Move(Motion::Down),
        'k' if tall => Deed::Move(Motion::Up),
        'j' => Deed::Scroll(Toward::LineDown),
        'k' => Deed::Scroll(Toward::LineUp),
        'w' | 'W' => Deed::Move(Motion::WordRight),
        'b' | 'B' => Deed::Move(Motion::WordLeft),
        'e' | 'E' => Deed::Move(Motion::WordEnd),
        '0' => Deed::Move(Motion::LineStart),
        '^' | '_' => Deed::Move(Motion::FirstWord),
        '$' => Deed::Move(Motion::LineEnd),
        'G' if tall => Deed::Move(Motion::Last),
        'G' => Deed::Scroll(Toward::Bottom),

        'f' => Deed::Await(Wants::Find),
        't' => Deed::Await(Wants::Till),
        'F' => Deed::Await(Wants::FindBack),
        'T' => Deed::Await(Wants::TillBack),
        'r' => Deed::Await(Wants::Replace),

        'd' => Deed::Operate(Operator::Delete),
        'c' => Deed::Operate(Operator::Change),
        'y' => Deed::Operate(Operator::Yank),

        'x' => Deed::Edit(Edit::DeleteChar),
        'X' => Deed::Edit(Edit::DeleteBack),
        'D' => Deed::Edit(Edit::KillToEnd),
        'p' => Deed::Edit(Edit::PasteAfter),
        'P' => Deed::Edit(Edit::PasteBefore),
        'J' => Deed::Edit(Edit::Join),
        '~' => Deed::Edit(Edit::FlipCase),
        'u' => Deed::Undo,

        ':' => Deed::Command,
        '/' => Deed::Search,
        'n' => Deed::Match { forward: true },
        'N' => Deed::Match { forward: false },
        _ => Deed::Unbound,
    }
}

/// The second key of `gg`, and of nothing else yet.
#[must_use]
pub fn after_g(key: char, tall: bool) -> Deed {
    match key {
        'g' if tall => Deed::Move(Motion::First),
        'g' => Deed::Scroll(Toward::Top),
        _ => Deed::Unbound,
    }
}

/// Whether a change made under this operator should take whole lines.
///
/// `dd`, `cc` and `yy`: the operator doubled. Everything else covers the ground a motion does.
#[must_use]
pub fn doubled(operator: Operator, key: char) -> bool {
    operator.key() == key
}

/// Move the cursor, and say whether the motion found anywhere to go.
///
/// The answer matters for `f` and `t`: `df,` on a line with no comma must leave the line alone
/// rather than deleting to wherever the cursor happened to stop.
pub fn travel(motion: Motion, editor: &mut crate::Editor) -> bool {
    match motion {
        Motion::Left => editor.left(),
        Motion::Right => editor.right(),
        Motion::Down => editor.down(),
        Motion::Up => editor.up(),
        Motion::WordRight => editor.word_right(),
        Motion::WordLeft => editor.word_left(),
        Motion::WordEnd => editor.word_end(),
        Motion::LineStart => editor.home(),
        Motion::FirstWord => editor.first_word(),
        Motion::LineEnd => editor.end(),
        Motion::First => editor.first_line(),
        Motion::Last => editor.last_line(),
        Motion::ToChar {
            target,
            forward,
            short,
        } => {
            return if forward {
                editor.find_char(target, short)
            } else {
                editor.find_char_back(target, short)
            };
        }
    }
    true
}

/// Apply a change that needs no motion.
pub fn apply(edit: Edit, editor: &mut crate::Editor) {
    match edit {
        Edit::DeleteChar => editor.delete_char(),
        Edit::DeleteBack => {
            editor.left();
            editor.delete_char();
        }
        Edit::DeleteLine => editor.delete_line(),
        Edit::YankLine => {
            let (row, _) = editor.cursor();
            editor.copy_lines(row, row);
        }
        Edit::KillToEnd => editor.kill_to_end(),
        Edit::PasteAfter => editor.paste(true),
        Edit::PasteBefore => editor.paste(false),
        Edit::OpenBelow => editor.open_below(),
        Edit::OpenAbove => editor.open_above(),
        Edit::Join => editor.join(),
        Edit::FlipCase => editor.flip_case(),
        Edit::Replace(c) => editor.replace_char(c),
        Edit::Go(motion) => {
            travel(motion, editor);
        }
    }
}

/// Whether an edit changes the buffer, and so is worth being able to undo.
#[must_use]
pub fn changes(edit: Edit) -> bool {
    !matches!(edit, Edit::Go(_) | Edit::YankLine)
}

/// The vocabulary: what each key is, and that the pieces compose.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_opens_in_normal_mode() {
        // The whole point. A modal editor that starts in insert mode is a non-modal editor
        // with an extra key to press.
        assert_eq!(Mode::default(), Mode::Normal);
        assert!(!Mode::default().is_insert());
    }

    #[test]
    fn the_command_line_takes_text_like_insert_mode_does() {
        // It is a place you type, so a printable key belongs in it. What makes it a mode of its
        // own is where the text goes, not whether keys are text.
        assert!(Mode::Command.is_insert());
    }

    #[test]
    fn every_tag_is_three_letters_wide() {
        // They sit on the prompt's border, and a tag that changed width would move the border
        // every time the mode did.
        for mode in [Mode::Normal, Mode::Insert, Mode::Command] {
            assert_eq!(mode.tag().chars().count(), 3, "{mode:?}");
        }
    }

    #[test]
    fn letters_are_commands_and_not_text() {
        // `i` is not an `i`. Every printable key that reaches normal mode either does something
        // or does nothing, and none of them land in the buffer.
        for key in "hjklwbeWBE0^$xXDdcypPJ~uiaIAoOCSnNGftFTr:/".chars() {
            assert_ne!(deed(key, false), Deed::Unbound, "{key:?} does nothing");
        }
    }

    #[test]
    fn the_operators_wait_for_a_motion() {
        for (key, operator) in [
            ('d', Operator::Delete),
            ('c', Operator::Change),
            ('y', Operator::Yank),
        ] {
            assert_eq!(deed(key, false), Deed::Operate(operator));
            assert!(doubled(operator, key), "and doubling takes the line");
            assert!(!doubled(operator, 'w'), "while a motion does not");
        }
    }

    #[test]
    fn the_find_keys_wait_for_a_character() {
        assert_eq!(deed('f', false), Deed::Await(Wants::Find));
        assert_eq!(
            Wants::Till.given(','),
            Deed::Move(Motion::ToChar {
                target: ',',
                forward: true,
                short: true
            })
        );
        assert_eq!(Wants::Replace.given('z'), Deed::Edit(Edit::Replace('z')));
    }

    #[test]
    fn j_and_k_move_in_a_tall_prompt_and_scroll_a_short_one() {
        // The one binding that depends on anything but the key. A single-line prompt has
        // nowhere to move up or down to, and somebody reading back through output wants these.
        assert_eq!(deed('j', true), Deed::Move(Motion::Down));
        assert_eq!(deed('j', false), Deed::Scroll(Toward::LineDown));
        assert_eq!(deed('G', true), Deed::Move(Motion::Last));
        assert_eq!(deed('G', false), Deed::Scroll(Toward::Bottom));
        assert_eq!(after_g('g', true), Deed::Move(Motion::First));
        assert_eq!(after_g('g', false), Deed::Scroll(Toward::Top));
    }

    #[test]
    fn a_half_typed_command_is_abandoned_rather_than_guessed_at() {
        // `gw` is not a command, and doing *something* on the grounds that two keys were
        // pressed is how an editor eats a line nobody asked it to.
        assert_eq!(after_g('w', false), Deed::Unbound);
    }

    #[test]
    fn every_motion_reaches_the_editor() {
        // A variant added here and not wired into `travel` is a key that does nothing, which is
        // indistinguishable from an unbound one until somebody presses it.
        let every = [
            Motion::Left,
            Motion::Right,
            Motion::Down,
            Motion::Up,
            Motion::WordRight,
            Motion::WordLeft,
            Motion::WordEnd,
            Motion::LineStart,
            Motion::FirstWord,
            Motion::LineEnd,
            Motion::First,
            Motion::Last,
            Motion::ToChar {
                target: 'o',
                forward: true,
                short: false,
            },
        ];
        for motion in every {
            let mut editor = crate::Editor::new();
            editor.insert_str("one two\nthree four");
            travel(motion, &mut editor);
        }
    }

    #[test]
    fn every_edit_reaches_the_editor() {
        let every = [
            Edit::DeleteChar,
            Edit::DeleteBack,
            Edit::DeleteLine,
            Edit::YankLine,
            Edit::KillToEnd,
            Edit::PasteAfter,
            Edit::PasteBefore,
            Edit::OpenBelow,
            Edit::OpenAbove,
            Edit::Join,
            Edit::FlipCase,
            Edit::Replace('z'),
            Edit::Go(Motion::LineEnd),
        ];
        for edit in every {
            let mut editor = crate::Editor::new();
            editor.insert_str("one two\nthree four");
            apply(edit, &mut editor);
        }
    }

    #[test]
    fn a_motion_that_finds_nothing_says_so() {
        // `df,` on a line with no comma has to leave the line alone rather than deleting to
        // wherever the cursor stopped.
        let mut editor = crate::Editor::new();
        editor.insert_str("no commas here");
        editor.home();
        assert!(!travel(
            Motion::ToChar {
                target: ',',
                forward: true,
                short: false
            },
            &mut editor
        ));
        assert_eq!(editor.cursor().1, 0, "and it did not move");
    }

    #[test]
    fn moving_about_is_not_a_change() {
        // `u` walks back through edits. A motion in the undo stack is a keypress you have to
        // press `u` twice to get past, for no change you can see.
        assert!(!changes(Edit::Go(Motion::LineEnd)));
        assert!(!changes(Edit::YankLine), "copying leaves the buffer alone");
        assert!(changes(Edit::DeleteLine));
        assert!(changes(Edit::Replace('z')));
    }
}
