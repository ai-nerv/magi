//! Modes.
//!
//! The prompt is modal: it opens in normal mode and does not take text until you ask it to.
//! That is the whole point of the thing — a key press in normal mode is a command, and typing
//! into a buffer is one command among the rest rather than the default.
//!
//! There is no setting for it. A modal editor that can be switched off is two editors to keep
//! working, and the second one is the one nobody tests.
//!
//! What lives here is the *vocabulary*: which mode the prompt is in, and what a key means in
//! normal mode. Applying that to a buffer is [`crate::editor`]'s job and deciding what a
//! non-editing command does — scrolling, submitting, searching — belongs to the caller, which
//! is the only thing that knows there is a transcript.

/// Which mode the prompt is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Keys are commands. Where the prompt opens, and where `esc` returns it.
    #[default]
    Normal,
    /// Keys are text.
    Insert,
}

impl Mode {
    /// The three letters shown on the prompt box.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Normal => "NOR",
            Self::Insert => "INS",
        }
    }

    /// Whether typing a printable character puts it in the buffer.
    #[must_use]
    pub fn is_insert(self) -> bool {
        self == Self::Insert
    }
}

/// What a key means in normal mode.
///
/// Motions and edits are separated from everything else because the first two are the editor's
/// and the rest are the caller's: `Scroll` needs a transcript, `Submit` needs a daemon, and
/// neither is something a text buffer should know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deed {
    /// Move the cursor, or change the buffer. Applied to the editor.
    Edit(Edit),
    /// Enter insert mode, having first done `Edit` — `a` moves right, `o` opens a line.
    Insert(Option<Edit>),
    /// Move the transcript.
    Scroll(Toward),
    /// Start a command line, by typing `:` in insert mode.
    Command,
    /// Start a search.
    Search,
    /// Go to the next or previous match.
    Match { forward: bool },
    /// Send what is in the buffer.
    Submit,
    /// Nothing is bound to this key.
    Unbound,
}

/// A change to the buffer, or a move within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
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
    /// `0`
    LineStart,
    /// `^`
    FirstWord,
    /// `$`
    LineEnd,
    /// `x`
    DeleteChar,
    /// `dd`
    DeleteLine,
    /// `D`
    KillToEnd,
    /// `p`
    Paste,
    /// `o`
    OpenBelow,
    /// `O`
    OpenAbove,
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
    /// `gg`
    Top,
    /// `G`
    Bottom,
}

/// A key in normal mode, given what came before it and how tall the prompt is.
///
/// `pending` is the half-typed operator — the first `d` of `dd`, the first `g` of `gg` — and is
/// the caller's to keep, because a key handler that remembered would have to be told when the
/// buffer changed under it.
///
/// `tall` decides what `j` and `k` mean, and it is the one genuinely awkward binding here. A
/// single-line prompt has nowhere to move up or down *to*, so they scroll the transcript, which
/// is what somebody reading back through output wants them to do. A prompt with several lines
/// has somewhere to go, so they go there. The arrow keys do the same thing for the same reason.
#[must_use]
pub fn deed(key: char, pending: Option<char>, tall: bool) -> Deed {
    if let Some(waiting) = pending {
        return match (waiting, key) {
            ('d', 'd') => Deed::Edit(Edit::DeleteLine),
            ('g', 'g') => Deed::Scroll(Toward::Top),
            // `dk`, `gw`, anything else: the operator is abandoned rather than guessed at.
            _ => Deed::Unbound,
        };
    }
    match key {
        'i' => Deed::Insert(None),
        'a' => Deed::Insert(Some(Edit::Right)),
        'I' => Deed::Insert(Some(Edit::FirstWord)),
        'A' => Deed::Insert(Some(Edit::LineEnd)),
        'o' => Deed::Insert(Some(Edit::OpenBelow)),
        'O' => Deed::Insert(Some(Edit::OpenAbove)),
        // Change: the kill half is an edit, the insert half is the mode.
        'C' => Deed::Insert(Some(Edit::KillToEnd)),
        'S' => Deed::Insert(Some(Edit::DeleteLine)),

        'h' => Deed::Edit(Edit::Left),
        'l' => Deed::Edit(Edit::Right),
        'j' if tall => Deed::Edit(Edit::Down),
        'k' if tall => Deed::Edit(Edit::Up),
        'j' => Deed::Scroll(Toward::LineDown),
        'k' => Deed::Scroll(Toward::LineUp),
        'w' => Deed::Edit(Edit::WordRight),
        'b' => Deed::Edit(Edit::WordLeft),
        '0' => Deed::Edit(Edit::LineStart),
        '^' => Deed::Edit(Edit::FirstWord),
        '$' => Deed::Edit(Edit::LineEnd),
        'G' => Deed::Scroll(Toward::Bottom),

        'x' => Deed::Edit(Edit::DeleteChar),
        'D' => Deed::Edit(Edit::KillToEnd),
        'p' => Deed::Edit(Edit::Paste),

        ':' => Deed::Command,
        '/' => Deed::Search,
        'n' => Deed::Match { forward: true },
        'N' => Deed::Match { forward: false },
        _ => Deed::Unbound,
    }
}

/// Whether this key starts a two-key command and should be held rather than acted on.
#[must_use]
pub fn holds(key: char) -> bool {
    matches!(key, 'd' | 'g')
}

/// Apply a normal-mode edit to a buffer.
///
/// Here rather than in the editor because these are vim's names for things the editor already
/// does — the editor has no opinion about what `w` is called.
pub fn apply(edit: Edit, editor: &mut crate::Editor) {
    match edit {
        Edit::Left => editor.left(),
        Edit::Right => editor.right(),
        Edit::Down => editor.down(),
        Edit::Up => editor.up(),
        Edit::WordRight => editor.word_right(),
        Edit::WordLeft => editor.word_left(),
        Edit::LineStart => editor.home(),
        Edit::FirstWord => editor.first_word(),
        Edit::LineEnd => editor.end(),
        Edit::DeleteChar => editor.delete_char(),
        Edit::DeleteLine => editor.delete_line(),
        Edit::KillToEnd => editor.kill_to_end(),
        Edit::Paste => editor.yank(),
        Edit::OpenBelow => editor.open_below(),
        Edit::OpenAbove => editor.open_above(),
    }
}

/// The prompt opens in normal mode, and every key in it is a command.
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
    fn letters_are_commands_and_not_text() {
        // `i` is not an `i`. Every printable key that reaches normal mode either does
        // something or does nothing, and none of them land in the buffer.
        for key in "hjklwb0^$xDpiaIAoOCSnNG:/".chars() {
            assert_ne!(
                deed(key, None, false),
                Deed::Unbound,
                "{key:?} does nothing"
            );
        }
    }

    #[test]
    fn the_ways_into_insert_mode_say_where_they_land() {
        assert_eq!(deed('i', None, false), Deed::Insert(None));
        assert_eq!(deed('a', None, false), Deed::Insert(Some(Edit::Right)));
        assert_eq!(deed('A', None, false), Deed::Insert(Some(Edit::LineEnd)));
        assert_eq!(deed('o', None, false), Deed::Insert(Some(Edit::OpenBelow)));
    }

    #[test]
    fn j_and_k_move_in_a_tall_prompt_and_scroll_a_short_one() {
        // The one binding here that depends on anything but the key. A single-line prompt has
        // nowhere to move up or down to, and somebody reading back through output wants these.
        assert_eq!(deed('j', None, true), Deed::Edit(Edit::Down));
        assert_eq!(deed('k', None, true), Deed::Edit(Edit::Up));
        assert_eq!(deed('j', None, false), Deed::Scroll(Toward::LineDown));
        assert_eq!(deed('k', None, false), Deed::Scroll(Toward::LineUp));
    }

    #[test]
    fn two_key_commands_wait_for_their_second_key() {
        assert!(holds('d'), "d starts dd");
        assert!(holds('g'), "g starts gg");
        assert_eq!(deed('d', Some('d'), false), Deed::Edit(Edit::DeleteLine));
        assert_eq!(deed('g', Some('g'), false), Deed::Scroll(Toward::Top));
    }

    #[test]
    fn a_half_typed_command_is_abandoned_rather_than_guessed_at() {
        // `dk` is a motion-delete this does not have, and doing *something* on the grounds
        // that two keys were pressed is how an editor eats a line nobody asked it to.
        assert_eq!(deed('k', Some('d'), false), Deed::Unbound);
        assert_eq!(deed('w', Some('g'), false), Deed::Unbound);
    }

    #[test]
    fn the_prefixes_are_what_they_used_to_be() {
        // `:` opens the command line and `/` searches. They were one key doing both jobs, and
        // a prefix cannot mean two things.
        assert_eq!(deed(':', None, false), Deed::Command);
        assert_eq!(deed('/', None, false), Deed::Search);
    }

    #[test]
    fn every_edit_reaches_the_editor() {
        // A variant added here and not wired into `apply` is a key that does nothing, which is
        // indistinguishable from an unbound one until somebody presses it.
        let every = [
            Edit::Left,
            Edit::Right,
            Edit::Down,
            Edit::Up,
            Edit::WordRight,
            Edit::WordLeft,
            Edit::LineStart,
            Edit::FirstWord,
            Edit::LineEnd,
            Edit::DeleteChar,
            Edit::DeleteLine,
            Edit::KillToEnd,
            Edit::Paste,
            Edit::OpenBelow,
            Edit::OpenAbove,
        ];
        for edit in every {
            let mut editor = crate::Editor::new();
            editor.insert_str("one two\nthree four");
            apply(edit, &mut editor);
        }
    }

    #[test]
    fn the_tag_is_three_letters_wide() {
        // It sits on the prompt's border, and a tag that changed width would move the border
        // every time the mode changed.
        assert_eq!(Mode::Normal.tag().chars().count(), 3);
        assert_eq!(Mode::Insert.tag().chars().count(), 3);
    }
}
