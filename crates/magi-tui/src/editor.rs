//! The prompt editor.
//!
//! Multi-line, with word navigation and a kill ring, because a coding prompt is a paragraph
//! and not a shell command. Enter submits; Shift+Enter inserts a newline, which is why the
//! Kitty keyboard protocol is negotiated at startup.

/// A multi-line text buffer with a cursor.
#[derive(Debug, Default)]
pub struct Editor {
    lines: Vec<String>,
    /// Line index of the cursor.
    row: usize,
    /// Character index within `lines[row]`, not a byte offset.
    col: usize,
    /// Most recently killed text, for `Ctrl-Y` and for `p`.
    kill_ring: String,
    /// Whether what is in the kill ring was taken as whole lines.
    ///
    /// vim's distinction, and it is not decoration: `yy` then `p` puts the line *below* the
    /// one you are on, while `yw` then `p` puts the word *after the cursor*. Without this the
    /// two are the same paste and one of them is always wrong.
    kill_lines: bool,
    /// Buffers to go back to, oldest first.
    undo: Vec<Snapshot>,
    /// Submitted prompts, oldest first.
    history: Vec<String>,
    /// Position while walking `history`; `None` means "editing, not browsing".
    history_pos: Option<usize>,
    /// Characters typed a moment ago, for the reveal in [`crate::prompt`].
    typed: Vec<Typed>,
}

/// A character somebody has just typed: where it went, what it was, and when.
///
/// The character is kept as well as the position because an edit that moves text around leaves
/// the positions pointing at somebody else's letters. Checking what is actually there is cheaper
/// than invalidating this from every method that can shift a line.
#[derive(Debug, Clone, Copy)]
struct Typed {
    row: usize,
    col: usize,
    ch: char,
    at: std::time::Instant,
}

/// A buffer as it stood, to go back to.
#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

/// How many edits back you can go.
///
/// Bounded because a session is long and every keystroke in normal mode can be an edit. Deep
/// enough that nobody reaches the end of it while still remembering what they did.
const UNDOS: usize = 200;

/// How long a typed character is remembered, whatever the reveal is set to.
const REMEMBERED: std::time::Duration = std::time::Duration::from_secs(1);

/// How many are remembered at once, so a paste does not grow this without bound.
const RECENT: usize = 64;

impl Editor {
    /// An empty editor with one blank line.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            ..Self::default()
        }
    }

    /// The same, starting with the prompts from previous runs.
    ///
    /// The arrow keys have walked a history since M2 and the history started empty every run, so
    /// it worked within one session and had nothing in it the moment you came back — which is
    /// when the prompt you want again is the one from yesterday.
    #[must_use]
    pub fn with_history(history: Vec<String>) -> Self {
        Self {
            lines: vec![String::new()],
            history,
            ..Self::default()
        }
    }

    /// The full text, lines joined by newlines.
    #[must_use]
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether the buffer holds nothing but whitespace.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    /// The buffer's lines, for rendering.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Cursor position as `(row, column)`, both zero-based, column in characters.
    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Empty the buffer and stop browsing history.
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
        self.history_pos = None;
    }

    /// Take the text, record it in history, and reset.
    ///
    /// Returns `None` for a blank buffer so Enter on an empty prompt does nothing.
    pub fn submit(&mut self) -> Option<String> {
        if self.is_blank() {
            return None;
        }
        let text = self.text();
        self.history.push(text.clone());
        self.clear();
        Some(text)
    }

    /// Insert a character at the cursor.
    pub fn insert(&mut self, c: char) {
        let byte = self.byte_offset();
        self.lines[self.row].insert(byte, c);
        self.col += 1;
        self.history_pos = None;
        self.note(c);
    }

    /// Remember that `c` was just typed, and forget anything too old to still be resolving.
    fn note(&mut self, c: char) {
        let at = std::time::Instant::now();
        self.typed.retain(|t| at.duration_since(t.at) < REMEMBERED);
        if self.typed.len() >= RECENT {
            self.typed.remove(0);
        }
        self.typed.push(Typed {
            row: self.row,
            col: self.col - 1,
            ch: c,
            at,
        });
    }

    /// How long ago the character at `row, col` was typed, if it still is what was typed there.
    #[must_use]
    pub fn typed_age(&self, row: usize, col: usize, ch: char) -> Option<std::time::Duration> {
        self.typed
            .iter()
            .rev()
            .find(|t| t.row == row && t.col == col && t.ch == ch)
            .map(|t| t.at.elapsed())
    }

    /// Insert text, splitting on newlines. Used for bracketed paste.
    pub fn insert_str(&mut self, text: &str) {
        for (i, chunk) in text.split('\n').enumerate() {
            if i > 0 {
                self.newline();
            }
            for c in chunk.chars() {
                self.insert(c);
            }
        }
    }

    /// Split the current line at the cursor.
    pub fn newline(&mut self) {
        let byte = self.byte_offset();
        let tail = self.lines[self.row].split_off(byte);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
        self.history_pos = None;
    }

    /// Delete the character before the cursor, joining lines at a line start.
    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let byte = self.byte_offset();
            self.lines[self.row].remove(byte);
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&current);
        }
        self.history_pos = None;
    }

    /// Move the cursor one character left, wrapping to the previous line.
    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    /// Move the cursor one character right, wrapping to the next line.
    pub fn right(&mut self) {
        if self.col < self.lines[self.row].chars().count() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// Move to the start of the line.
    pub fn home(&mut self) {
        self.col = 0;
    }

    /// Move the cursor one line up, keeping as much of its column as the line has.
    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    /// Move the cursor one line down, keeping as much of its column as the line has.
    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    /// Move to the first character on the line that is not a space.
    pub fn first_word(&mut self) {
        self.col = self.lines[self.row]
            .chars()
            .position(|c| !c.is_whitespace())
            .unwrap_or(0);
    }

    /// Move to the last line.
    pub fn last_line(&mut self) {
        self.row = self.lines.len() - 1;
        self.col = self.col.min(self.lines[self.row].chars().count());
    }

    /// Move to the first line.
    pub fn first_line(&mut self) {
        self.row = 0;
        self.col = self.col.min(self.lines[self.row].chars().count());
    }

    /// Delete the character under the cursor.
    ///
    /// Nothing at the end of a line: `x` in vim does not join lines, and a delete that
    /// silently pulled the next line up would be a different operator wearing the same key.
    pub fn delete_char(&mut self) {
        let byte = self.byte_offset();
        if byte < self.lines[self.row].len() {
            self.lines[self.row].remove(byte);
        }
        // Off the end after deleting the last character, which is where `x` leaves you.
        self.col = self.col.min(self.lines[self.row].chars().count());
    }

    /// Delete the whole line, into the kill ring.
    ///
    /// The last line is emptied rather than removed: a buffer with no lines has no cursor
    /// position, and every method here indexes `lines[row]`.
    pub fn delete_line(&mut self) {
        self.kill_ring = std::mem::take(&mut self.lines[self.row]);
        self.kill_lines = true;
        if self.lines.len() > 1 {
            self.lines.remove(self.row);
            self.row = self.row.min(self.lines.len() - 1);
        }
        self.col = 0;
    }

    /// Open a blank line below the cursor and put the cursor on it.
    pub fn open_below(&mut self) {
        self.lines.insert(self.row + 1, String::new());
        self.row += 1;
        self.col = 0;
    }

    /// Open a blank line above the cursor and put the cursor on it.
    pub fn open_above(&mut self) {
        self.lines.insert(self.row, String::new());
        self.col = 0;
    }

    /// How many lines the buffer holds.
    #[must_use]
    pub fn height(&self) -> usize {
        self.lines.len()
    }

    /// Pull the cursor back off the end of the line.
    ///
    /// Normal mode sits *on* a character rather than between two, so the column one past the
    /// end — where insert mode legitimately puts it — is not a place it can rest.
    pub fn settle(&mut self) {
        let last = self.lines[self.row].chars().count();
        self.col = self.col.min(last.saturating_sub(1));
    }

    /// Put the cursor somewhere, clamped to what is actually there.
    pub fn goto(&mut self, row: usize, col: usize) {
        self.row = row.min(self.lines.len() - 1);
        self.col = col.min(self.lines[self.row].chars().count());
    }

    /// Keep the buffer as it stands, to go back to.
    ///
    /// Called by whatever is about to change it rather than by the methods that do the
    /// changing: one command is one undo, and a command built out of three editor calls would
    /// otherwise take three `u` to walk back.
    pub fn remember(&mut self) {
        self.undo.push(Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        });
        if self.undo.len() > UNDOS {
            self.undo.remove(0);
        }
    }

    /// Go back to the buffer before the last remembered change.
    ///
    /// Answers whether there was anything to go back to, so a caller can say "already at the
    /// oldest change" rather than redrawing an unchanged screen.
    pub fn undo(&mut self) -> bool {
        let Some(was) = self.undo.pop() else {
            return false;
        };
        self.lines = was.lines;
        self.row = was.row;
        self.col = was.col;
        self.history_pos = None;
        true
    }

    /// The text between two positions, in buffer order.
    #[must_use]
    pub fn between(&self, from: (usize, usize), to: (usize, usize)) -> String {
        let (start, end) = if from <= to { (from, to) } else { (to, from) };
        if start.0 == end.0 {
            return self.lines[start.0]
                .chars()
                .skip(start.1)
                .take(end.1 - start.1)
                .collect();
        }
        let mut out: String = self.lines[start.0].chars().skip(start.1).collect();
        for row in start.0 + 1..end.0 {
            out.push('\n');
            out.push_str(&self.lines[row]);
        }
        out.push('\n');
        out.extend(self.lines[end.0].chars().take(end.1));
        out
    }

    /// Cut the text between two positions into the kill ring.
    pub fn cut(&mut self, from: (usize, usize), to: (usize, usize)) {
        let (start, end) = if from <= to { (from, to) } else { (to, from) };
        self.kill_ring = self.between(start, end);
        self.kill_lines = false;
        let head: String = self.lines[start.0].chars().take(start.1).collect();
        let tail: String = self.lines[end.0].chars().skip(end.1).collect();
        self.lines.splice(start.0..=end.0, [head + &tail]);
        self.row = start.0;
        self.col = start.1;
        self.history_pos = None;
    }

    /// Copy the text between two positions into the kill ring, leaving the buffer alone.
    pub fn copy(&mut self, from: (usize, usize), to: (usize, usize)) {
        self.kill_ring = self.between(from, to);
        self.kill_lines = false;
    }

    /// Copy whole lines into the kill ring, so a later paste opens a line for them.
    pub fn copy_lines(&mut self, from: usize, to: usize) {
        let (start, end) = (from.min(to), from.max(to));
        self.kill_ring = self.lines[start..=end.min(self.lines.len() - 1)].join("\n");
        self.kill_lines = true;
    }

    /// Put the kill ring back: after the cursor, or on a new line if it was taken as lines.
    pub fn paste(&mut self, after: bool) {
        let text = std::mem::take(&mut self.kill_ring);
        if self.kill_lines {
            let at = if after { self.row + 1 } else { self.row };
            for (index, line) in text.split('\n').enumerate() {
                self.lines.insert(at + index, line.to_owned());
            }
            self.row = at;
            self.col = 0;
        } else {
            if after && !self.lines[self.row].is_empty() {
                self.col = (self.col + 1).min(self.lines[self.row].chars().count());
            }
            self.insert_str(&text);
        }
        self.kill_ring = text;
        self.history_pos = None;
    }

    /// Move to the last character of the word the cursor is in, or of the next one.
    pub fn word_end(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        if self.col + 1 >= chars.len() {
            return;
        }
        self.col += 1;
        while self.col < chars.len() && chars[self.col].is_whitespace() {
            self.col += 1;
        }
        while self.col + 1 < chars.len() && !chars[self.col + 1].is_whitespace() {
            self.col += 1;
        }
    }

    /// Replace the character under the cursor.
    pub fn replace_char(&mut self, c: char) {
        let byte = self.byte_offset();
        if byte < self.lines[self.row].len() {
            let mut rest = self.lines[self.row].split_off(byte);
            let mut chars = rest.chars();
            chars.next();
            rest = chars.as_str().to_owned();
            self.lines[self.row].push(c);
            self.lines[self.row].push_str(&rest);
        }
        self.history_pos = None;
    }

    /// Swap the case of the character under the cursor, and step over it.
    pub fn flip_case(&mut self) {
        let Some(c) = self.lines[self.row].chars().nth(self.col) else {
            return;
        };
        let swapped = if c.is_uppercase() {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c.to_uppercase().next().unwrap_or(c)
        };
        self.replace_char(swapped);
        self.right();
    }

    /// Pull the next line onto the end of this one, with a space between.
    pub fn join(&mut self) {
        if self.row + 1 >= self.lines.len() {
            return;
        }
        let next = self.lines.remove(self.row + 1);
        let joined = next.trim_start();
        self.col = self.lines[self.row].chars().count();
        if !self.lines[self.row].is_empty() && !joined.is_empty() {
            self.lines[self.row].push(' ');
        }
        self.lines[self.row].push_str(joined);
        self.history_pos = None;
    }

    /// Move to the next occurrence of `c` on this line, or to just before it.
    ///
    /// Answers whether it found one, so `df,` on a line with no comma leaves the line alone
    /// rather than deleting to the end of it.
    pub fn find_char(&mut self, c: char, before: bool) -> bool {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let found = chars
            .iter()
            .enumerate()
            .skip(self.col + 1)
            .find(|(_, at)| **at == c)
            .map(|(index, _)| index);
        let Some(index) = found else {
            return false;
        };
        self.col = if before {
            index.saturating_sub(1)
        } else {
            index
        };
        true
    }

    /// The same, backwards.
    pub fn find_char_back(&mut self, c: char, after: bool) -> bool {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let found = chars
            .iter()
            .enumerate()
            .take(self.col)
            .rev()
            .find(|(_, at)| **at == c)
            .map(|(index, _)| index);
        let Some(index) = found else {
            return false;
        };
        self.col = if after { index + 1 } else { index };
        true
    }

    /// Move to the end of the line.
    pub fn end(&mut self) {
        self.col = self.lines[self.row].chars().count();
    }

    /// Move left by one word.
    pub fn word_left(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        while self.col > 0 && chars[self.col - 1].is_whitespace() {
            self.col -= 1;
        }
        while self.col > 0 && !chars[self.col - 1].is_whitespace() {
            self.col -= 1;
        }
    }

    /// Move right by one word.
    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        while self.col < chars.len() && !chars[self.col].is_whitespace() {
            self.col += 1;
        }
        while self.col < chars.len() && chars[self.col].is_whitespace() {
            self.col += 1;
        }
    }

    /// Kill from the cursor to the end of the line, into the kill ring.
    pub fn kill_to_end(&mut self) {
        let byte = self.byte_offset();
        self.kill_ring = self.lines[self.row].split_off(byte);
    }

    /// Kill from the start of the line to the cursor, into the kill ring.
    pub fn kill_to_start(&mut self) {
        let byte = self.byte_offset();
        let tail = self.lines[self.row].split_off(byte);
        self.kill_ring = std::mem::replace(&mut self.lines[self.row], tail);
        self.col = 0;
    }

    /// Insert the kill ring at the cursor.
    pub fn yank(&mut self) {
        let text = std::mem::take(&mut self.kill_ring);
        self.insert_str(&text);
        self.kill_ring = text;
    }

    /// Replace the buffer with the previous history entry.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.load_history(next);
    }

    /// Replace the buffer with the next history entry, or clear past the newest.
    pub fn history_next(&mut self) {
        let Some(pos) = self.history_pos else {
            return;
        };
        if pos + 1 >= self.history.len() {
            self.clear();
            return;
        }
        self.load_history(pos + 1);
    }

    fn load_history(&mut self, index: usize) {
        self.lines = self.history[index].split('\n').map(str::to_owned).collect();
        self.history_pos = Some(index);
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
    }

    /// Replace the characters from `start` to the cursor with `text`.
    ///
    /// Used when a completion is accepted: the token the popup was built from is exactly the
    /// span between where it began and where the cursor sits now.
    pub fn replace_token(&mut self, start: usize, text: &str) {
        let start = start.min(self.col);
        let from = self.byte_at(start);
        let to = self.byte_offset();
        self.lines[self.row].replace_range(from..to, text);
        self.col = start + text.chars().count();
        self.history_pos = None;
    }

    /// Replace the whole buffer, putting the cursor at the end.
    ///
    /// The external-editor round trip lands here: the file is authoritative on return.
    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_owned).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
        self.history_pos = None;
    }

    /// Byte offset of character index `index` within the cursor's line.
    fn byte_at(&self, index: usize) -> usize {
        self.lines[self.row]
            .char_indices()
            .nth(index)
            .map_or(self.lines[self.row].len(), |(i, _)| i)
    }

    /// Byte offset of the cursor within its line.
    ///
    /// `col` counts characters so cursor motion is uniform across scripts; indexing a `String`
    /// needs bytes, and the two differ the moment a prompt contains anything non-ASCII.
    fn byte_offset(&self) -> usize {
        self.lines[self.row]
            .char_indices()
            .nth(self.col)
            .map_or(self.lines[self.row].len(), |(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with(text: &str) -> Editor {
        let mut e = Editor::new();
        e.insert_str(text);
        e
    }

    #[test]
    fn typing_accumulates() {
        let e = editor_with("hello");
        assert_eq!(e.text(), "hello");
        assert_eq!(e.cursor(), (0, 5));
    }

    #[test]
    fn newlines_split_the_buffer() {
        let e = editor_with("a\nb");
        assert_eq!(e.lines(), ["a", "b"]);
        assert_eq!(e.cursor(), (1, 1));
    }

    #[test]
    fn backspace_at_a_line_start_joins_lines() {
        let mut e = editor_with("a\nb");
        e.home();
        e.backspace();
        assert_eq!(e.text(), "ab");
        assert_eq!(e.cursor(), (0, 1));
    }

    #[test]
    fn submitting_a_blank_buffer_yields_nothing() {
        let mut e = editor_with("   ");
        assert_eq!(e.submit(), None);
    }

    #[test]
    fn submitting_clears_and_records_history() {
        let mut e = editor_with("run tests");
        assert_eq!(e.submit().as_deref(), Some("run tests"));
        assert_eq!(e.text(), "");
        e.history_prev();
        assert_eq!(e.text(), "run tests");
    }

    #[test]
    fn history_next_past_the_newest_clears() {
        let mut e = editor_with("one");
        e.submit();
        e.history_prev();
        e.history_next();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn word_motion_crosses_whitespace() {
        let mut e = editor_with("alpha beta");
        e.word_left();
        assert_eq!(e.cursor(), (0, 6));
        e.word_left();
        assert_eq!(e.cursor(), (0, 0));
    }

    #[test]
    fn kill_and_yank_round_trip() {
        let mut e = editor_with("hello world");
        e.home();
        e.word_right();
        e.kill_to_end();
        assert_eq!(e.text(), "hello ");
        e.yank();
        assert_eq!(e.text(), "hello world");
    }

    #[test]
    fn replacing_a_token_swaps_the_span_before_the_cursor() {
        let mut e = editor_with("look at @src");
        e.replace_token(8, "src/main.rs");
        assert_eq!(e.text(), "look at src/main.rs");
        assert_eq!(e.cursor(), (0, 19));
    }

    #[test]
    fn replacing_a_token_works_past_multibyte_text() {
        let mut e = editor_with("café @sr");
        e.replace_token(5, "src");
        assert_eq!(e.text(), "café src");
    }

    #[test]
    fn set_text_replaces_the_buffer_and_parks_the_cursor_at_the_end() {
        let mut e = editor_with("old");
        e.set_text("new\nlines");
        assert_eq!(e.lines(), ["new", "lines"]);
        assert_eq!(e.cursor(), (1, 5));
    }

    #[test]
    fn set_text_on_empty_input_leaves_one_blank_line() {
        let mut e = editor_with("old");
        e.set_text("");
        assert_eq!(e.lines(), [""]);
    }

    #[test]
    fn the_cursor_indexes_characters_not_bytes() {
        let mut e = editor_with("héllo");
        e.home();
        e.right();
        e.right();
        e.insert('X');
        assert_eq!(e.text(), "héXllo");
    }
}
