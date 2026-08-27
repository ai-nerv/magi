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
    /// Most recently killed text, for `Ctrl-Y`.
    kill_ring: String,
    /// Submitted prompts, oldest first.
    history: Vec<String>,
    /// Position while walking `history`; `None` means "editing, not browsing".
    history_pos: Option<usize>,
}

impl Editor {
    /// An empty editor with one blank line.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
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
