//! Where the reader is in the transcript: which of its rows are on screen, and whether new output
//! keeps the view at the end. The rows themselves are the stack's; see
//! [`crate::transcript::Stack`].

use std::ops::Range;

/// The reader's place in a transcript of `len` rows.
#[derive(Default)]
pub struct Scrollback {
    len: usize,
    /// Index of the first visible row.
    offset: usize,
    /// Whether new content should keep the view pinned to the end.
    ///
    /// Set while the reader is at the bottom and cleared the moment they scroll away, so
    /// arriving output never yanks the view out from under someone reading history.
    follow: bool,
}

impl Scrollback {
    /// An empty transcript, following the tail.
    #[must_use]
    pub fn new() -> Self {
        Self {
            len: 0,
            offset: 0,
            follow: true,
        }
    }

    /// How many rows the transcript holds now.
    pub fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    /// How many rows the transcript holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the transcript is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the view is pinned to the newest output.
    #[must_use]
    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// How many rows sit below the current view.
    ///
    /// Zero while following. A reader who has scrolled up sees a screen that looks exactly
    /// like the bottom of the transcript, and output arriving below it makes no sound.
    #[must_use]
    pub fn hidden_below(&self, height: u16) -> usize {
        self.len.saturating_sub(self.offset + usize::from(height))
    }

    /// How many rows sit above the current view.
    #[must_use]
    pub fn hidden_above(&self) -> usize {
        self.offset
    }

    /// The rows visible in a viewport `height` rows tall.
    pub fn view(&mut self, height: u16) -> Range<usize> {
        let height = usize::from(height);
        let max = self.len.saturating_sub(height);
        if self.follow {
            self.offset = max;
        } else {
            self.offset = self.offset.min(max);
        }
        self.offset..(self.offset + height).min(self.len)
    }

    /// Scroll towards the start of the transcript.
    pub fn scroll_up(&mut self, rows: usize) {
        self.offset = self.offset.saturating_sub(rows);
        self.follow = false;
    }

    /// Scroll towards the end, resuming follow on arrival.
    pub fn scroll_down(&mut self, rows: usize, height: u16) {
        let max = self.len.saturating_sub(usize::from(height));
        self.offset = (self.offset + rows).min(max);
        if self.offset >= max {
            self.follow = true;
        }
    }

    /// Scroll up by most of a screen.
    pub fn page_up(&mut self, height: u16) {
        self.scroll_up(page(height));
    }

    /// Scroll down by most of a screen.
    pub fn page_down(&mut self, height: u16) {
        self.scroll_down(page(height), height);
    }

    /// Jump to the first row.
    pub fn to_top(&mut self) {
        self.offset = 0;
        self.follow = false;
    }

    /// Jump to the newest output and stay there.
    pub fn to_bottom(&mut self) {
        self.follow = true;
    }
}

/// Rows a page key moves, leaving some overlap so the reader keeps their place.
fn page(height: u16) -> usize {
    usize::from(crate::metric::share(height, crate::metric::page_share()))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(super) fn filled(n: usize) -> Scrollback {
        let mut buffer = Scrollback::new();
        buffer.set_len(n);
        buffer
    }

    #[test]
    fn a_new_buffer_follows_the_tail() {
        let mut buffer = filled(20);
        assert_eq!(buffer.view(3), 17..20);
    }

    #[test]
    fn content_shorter_than_the_viewport_shows_whole() {
        let mut buffer = filled(2);
        assert_eq!(buffer.view(10), 0..2);
    }

    #[test]
    fn scrolling_up_stops_following() {
        let mut buffer = filled(20);
        buffer.view(5);
        buffer.scroll_up(3);
        assert!(!buffer.is_following());
        assert_eq!(buffer.view(5).start, 12);
    }

    #[test]
    fn new_output_does_not_move_a_reader_who_scrolled_away() {
        let mut buffer = filled(20);
        buffer.view(5);
        buffer.scroll_up(5);
        let before = buffer.view(5);
        buffer.set_len(40);
        assert_eq!(buffer.view(5), before, "the view stayed put");
    }

    #[test]
    fn new_output_follows_for_a_reader_at_the_bottom() {
        let mut buffer = filled(20);
        buffer.view(5);
        buffer.set_len(40);
        assert_eq!(buffer.view(5).end, 40);
    }

    #[test]
    fn scrolling_back_to_the_bottom_resumes_following() {
        let mut buffer = filled(20);
        buffer.view(5);
        buffer.scroll_up(10);
        assert!(!buffer.is_following());
        buffer.scroll_down(10, 5);
        assert!(buffer.is_following());
    }

    #[test]
    fn a_page_is_half_a_screen() {
        let mut buffer = filled(100);
        buffer.view(10);
        buffer.page_up(10);
        assert_eq!(buffer.view(10).start, 85);
    }

    #[test]
    fn top_and_bottom_jump_all_the_way() {
        let mut buffer = filled(50);
        buffer.view(5);
        buffer.to_top();
        assert_eq!(buffer.view(5).start, 0);
        buffer.to_bottom();
        assert_eq!(buffer.view(5).end, 50);
    }

    #[test]
    fn scrolling_up_cannot_pass_the_start() {
        let mut buffer = filled(10);
        buffer.view(5);
        buffer.scroll_up(1000);
        assert_eq!(buffer.view(5).start, 0);
    }

    #[test]
    fn an_empty_buffer_yields_an_empty_view() {
        let mut buffer = Scrollback::new();
        assert!(buffer.view(10).is_empty());
        assert!(buffer.is_empty());
    }
}

#[cfg(test)]
mod hidden_tests {
    use super::tests::filled;

    #[test]
    fn following_the_tail_hides_nothing() {
        let mut s = filled(100);
        let _ = s.view(10);
        assert_eq!(s.hidden_below(10), 0);
    }

    #[test]
    fn scrolling_up_counts_what_is_below() {
        let mut s = filled(100);
        let _ = s.view(10);
        s.scroll_up(30);
        let _ = s.view(10);
        assert_eq!(s.hidden_below(10), 30);
    }

    #[test]
    fn coming_back_to_the_bottom_clears_it() {
        let mut s = filled(100);
        let _ = s.view(10);
        s.scroll_up(30);
        let _ = s.view(10);
        s.to_bottom();
        let _ = s.view(10);
        assert_eq!(s.hidden_below(10), 0);
    }

    #[test]
    fn a_transcript_shorter_than_the_screen_hides_nothing() {
        let mut s = filled(3);
        let _ = s.view(10);
        assert_eq!(s.hidden_below(10), 0);
    }
}

/// What a wheel notch does, which is what the driver hands straight to these.
#[cfg(test)]
mod wheel {
    use super::tests::filled;

    #[test]
    fn a_notch_up_leaves_the_tail_and_a_notch_back_returns_to_it() {
        let mut buffer = filled(100);
        buffer.view(10);
        assert!(
            buffer.is_following(),
            "a fresh buffer sits at the newest line"
        );
        buffer.scroll_up(3);
        assert!(
            !buffer.is_following(),
            "one notch is enough to stop following"
        );
        buffer.scroll_down(3, 10);
        assert!(buffer.is_following(), "and enough to come back");
    }

    #[test]
    fn a_wheel_spun_hard_stops_at_the_top_rather_than_running_off() {
        let mut buffer = filled(30);
        buffer.view(10);
        for _ in 0..200 {
            buffer.scroll_up(3);
        }
        let top = buffer.view(10);
        buffer.scroll_up(3);
        assert_eq!(buffer.view(10), top, "the top is the top");
        assert_eq!(top.start, 0, "and it is the first row");
    }
}
