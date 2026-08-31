//! A transcript buffer we own.
//!
//! The inline backend hands settled lines to the terminal and lets it keep the history. The
//! alt-screen backend has no terminal history to hand them to, so it keeps them here — which
//! is also what makes transcript search, selection, and jump-to-message possible later, none
//! of which can reach into a terminal's own scrollback.

use ratatui::text::Line;

/// The full transcript, plus where the reader is looking.
#[derive(Default)]
pub struct Scrollback {
    lines: Vec<Line<'static>>,
    /// Index of the first visible line.
    offset: usize,
    /// Whether new content should keep the view pinned to the end.
    ///
    /// Set while the reader is at the bottom and cleared the moment they scroll away, so
    /// arriving output never yanks the view out from under someone reading history.
    follow: bool,
}

impl Scrollback {
    /// An empty buffer, following the tail.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            offset: 0,
            follow: true,
        }
    }

    /// Replace the content.
    ///
    /// The whole transcript is re-rendered each frame rather than appended to, because a
    /// streaming message rewraps as it grows and its earlier lines are not final.
    pub fn set_lines(&mut self, lines: Vec<Line<'static>>) {
        self.lines = lines;
    }

    /// One line of the transcript, for a caller that has to know what is drawn where.
    ///
    /// A click is answered by what is under it rather than by a second list saying what ought
    /// to be: the fold handle is wherever the renderer put it, and asking the line is how that
    /// stays true when the renderer changes its mind.
    #[must_use]
    pub fn line(&self, at: usize) -> Option<&Line<'static>> {
        self.lines.get(at)
    }

    /// How many lines the transcript holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether the transcript is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Whether the view is pinned to the newest output.
    #[must_use]
    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// How many lines sit below the current view.
    ///
    /// Zero while following. A reader who has scrolled up sees a screen that looks exactly
    /// like the bottom of the transcript, and output arriving below it makes no sound.
    #[must_use]
    pub fn hidden_below(&self, height: u16) -> usize {
        self.lines
            .len()
            .saturating_sub(self.offset + usize::from(height))
    }

    /// The slice visible in a viewport `height` rows tall.
    pub fn view(&mut self, height: u16) -> &[Line<'static>] {
        let height = usize::from(height);
        let max = self.lines.len().saturating_sub(height);
        if self.follow {
            self.offset = max;
        } else {
            self.offset = self.offset.min(max);
        }
        let end = (self.offset + height).min(self.lines.len());
        &self.lines[self.offset..end]
    }

    /// Scroll towards the start of the transcript.
    pub fn scroll_up(&mut self, lines: usize) {
        self.offset = self.offset.saturating_sub(lines);
        self.follow = false;
    }

    /// Scroll towards the end, resuming follow on arrival.
    pub fn scroll_down(&mut self, lines: usize, height: u16) {
        let max = self.lines.len().saturating_sub(usize::from(height));
        self.offset = (self.offset + lines).min(max);
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

    /// Jump to the first line.
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
        buffer.set_lines((0..n).map(|i| Line::from(format!("line{i}"))).collect());
        buffer
    }

    pub(super) fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_new_buffer_follows_the_tail() {
        let mut buffer = filled(20);
        assert_eq!(texts(buffer.view(3)), ["line17", "line18", "line19"]);
    }

    #[test]
    fn content_shorter_than_the_viewport_shows_whole() {
        let mut buffer = filled(2);
        assert_eq!(texts(buffer.view(10)), ["line0", "line1"]);
    }

    #[test]
    fn scrolling_up_stops_following() {
        let mut buffer = filled(20);
        buffer.view(5);
        buffer.scroll_up(3);
        assert!(!buffer.is_following());
        assert_eq!(texts(buffer.view(5))[0], "line12");
    }

    #[test]
    fn new_output_does_not_move_a_reader_who_scrolled_away() {
        let mut buffer = filled(20);
        buffer.view(5);
        buffer.scroll_up(5);
        let before = texts(buffer.view(5));
        buffer.set_lines((0..40).map(|i| Line::from(format!("line{i}"))).collect());
        assert_eq!(texts(buffer.view(5)), before, "the view stayed put");
    }

    #[test]
    fn new_output_follows_for_a_reader_at_the_bottom() {
        let mut buffer = filled(20);
        buffer.view(5);
        buffer.set_lines((0..40).map(|i| Line::from(format!("line{i}"))).collect());
        assert_eq!(texts(buffer.view(5))[4], "line39");
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
        assert_eq!(texts(buffer.view(10))[0], "line85");
    }

    #[test]
    fn top_and_bottom_jump_all_the_way() {
        let mut buffer = filled(50);
        buffer.view(5);
        buffer.to_top();
        assert_eq!(texts(buffer.view(5))[0], "line0");
        buffer.to_bottom();
        assert_eq!(texts(buffer.view(5))[4], "line49");
    }

    #[test]
    fn scrolling_up_cannot_pass_the_start() {
        let mut buffer = filled(10);
        buffer.view(5);
        buffer.scroll_up(1000);
        assert_eq!(texts(buffer.view(5))[0], "line0");
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
    use super::*;
    use ratatui::text::Line;

    fn filled(n: usize) -> Scrollback {
        let mut s = Scrollback::new();
        s.set_lines((0..n).map(|i| Line::from(i.to_string())).collect());
        s
    }

    #[test]
    fn following_the_tail_hides_nothing() {
        let mut s = filled(100);
        let _ = s.view(10);
        assert_eq!(s.hidden_below(10), 0);
    }

    #[test]
    fn scrolling_up_counts_what_is_below() {
        // A scrolled screen looks exactly like the bottom of the transcript, and output
        // arriving below it makes no sound.
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
    use super::tests::{filled, texts};

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
        let top = texts(buffer.view(10)).to_vec();
        buffer.scroll_up(3);
        assert_eq!(texts(buffer.view(10)), top, "the top is the top");
        assert_eq!(top[0], "line0", "and it is the first line");
    }
}

impl Scrollback {
    /// How many lines sit above the current view.
    ///
    /// The other half of [`Self::hidden_below`], and the one nothing asked for until the
    /// transcript needed to say which edge it continues past.
    #[must_use]
    pub fn hidden_above(&self) -> usize {
        self.offset
    }
}
