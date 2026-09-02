//! The box writing to itself.
//!
//! An empty prompt sits there long enough and starts editing its own placeholder: it walks the
//! line, picks out a word or two, takes them out and writes different ones. It stops the instant
//! anybody touches a key.
//!
//! **It edits the way you would.** The ghost cursor is a bar while it is typing and a block when
//! it is not, it moves by words rather than sliding along a character at a time, and a change
//! shows you what it is about to take before it takes it. Not decoration: the prompt is a modal
//! editor, and the one thing that teaches a modal editor is watching one being used.
//!
//! # The engine
//!
//! A performance is a queue of [`Act`]s, each with a duration, played in order — so adding
//! something new is adding a variant and the place that writes one into a script. Nothing here
//! knows what a phrase is *about*; [`perform`] works out the difference between two lines and
//! writes the script that turns one into the other, and everything else just plays it.

#[cfg(test)]
#[path = "tease/going.rs"]
mod going_tests;

use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

/// How many lines back it remembers having shown.
///
/// Enough to walk out of a family of three or four before coming round again, and not so many
/// that a short pool runs out of things it is allowed to say.
const RECALLED: usize = 6;

/// One step of a performance.
///
/// Every act is a moment on screen, which is why each carries its own duration rather than
/// taking one from a setting here: a keystroke and a pause to read are different lengths of
/// time and the difference between them is the whole rhythm of the thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Do nothing, and be seen doing nothing.
    Rest(Duration),
    /// Change the ghost cursor's shape: a block for normal mode, a bar for insert.
    ///
    /// The *ghost's* shape. The prompt's own mode is not touched — this is a mime of one, and a
    /// box that changed the mode you were in to show you something would be a trap.
    Shape { block: bool, over: Duration },
    /// Put the ghost cursor here, in one jump, the way `w` and `b` move.
    Jump { to: usize, over: Duration },
    /// Invert a span, so what is about to go is visible before it goes.
    Mark { span: Range<usize>, over: Duration },
    /// Take the marked span out.
    Cut(Duration),
    /// Add one character at the cursor.
    Put { letter: char, over: Duration },
}

impl Act {
    /// How long this act is on screen.
    fn over(&self) -> Duration {
        match self {
            Self::Rest(over)
            | Self::Shape { over, .. }
            | Self::Jump { over, .. }
            | Self::Mark { over, .. }
            | Self::Cut(over)
            | Self::Put { over, .. } => *over,
        }
    }
}

/// What the box says about itself, beside whatever you have typed into it.
///
/// One value because a renderer handed these separately can be handed a caret for a line it is
/// not drawing, or a badge whose width nothing reserved.
#[derive(Debug, Clone, Default)]
pub struct Saying<'a> {
    /// The placeholder as it stands this frame, empty once anything is typed.
    pub text: &'a str,
    /// Where the box is editing its own placeholder, or `None` while it rests.
    pub caret: Option<usize>,
    /// Whether that cursor is a block, as it is when the box is not typing.
    pub block: bool,
    /// The span the box is about to take out, if it is showing you one.
    pub marked: Option<Range<usize>>,
    /// Which session this is, or its usage: drawn down the right.
    pub badge: &'a str,
    /// Which mode the prompt is in, drawn on its top edge.
    pub mode: crate::vim::Mode,
}

/// The box, and what it is in the middle of doing to itself.
#[derive(Debug)]
pub struct Tease {
    /// The line as it stands.
    shown: String,
    /// Where the ghost cursor is, in characters.
    caret: usize,
    /// Whether that cursor is a block.
    block: bool,
    /// The span currently inverted.
    marked: Option<Range<usize>>,
    /// Whether the ghost cursor is on screen at all.
    ///
    /// It arrives with the first performance and stays until somebody touches a key, which is
    /// the whole of its life: a second cursor on a prompt nobody has left alone yet is a second
    /// place to type, and a prompt has one.
    showing: bool,
    /// What is left to play.
    script: VecDeque<Act>,
    /// The lines already shown, newest last.
    ///
    /// Without this it never leaves: picking the closest line to the one on screen and nothing
    /// else means two lines in a family point at each other and it swaps between them forever.
    seen: VecDeque<String>,
    /// When the last act was played.
    since: Instant,
    /// How long its result is held before the next one.
    holding: Duration,
}

impl Tease {
    /// A box showing `opener` and doing nothing yet.
    #[must_use]
    pub fn new(opener: &str) -> Self {
        Self {
            shown: opener.to_owned(),
            caret: 0,
            block: false,
            marked: None,
            showing: false,
            script: VecDeque::new(),
            seen: VecDeque::from([opener.to_owned()]),
            since: Instant::now(),
            holding: Duration::ZERO,
        }
    }

    /// The line as it stands.
    #[must_use]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// What to draw this frame.
    #[must_use]
    pub fn saying(&self) -> Saying<'_> {
        Saying {
            text: &self.shown,
            caret: self.caret(),
            block: self.block,
            marked: self.marked.clone(),
            badge: "",
            mode: crate::vim::Mode::default(),
        }
    }

    /// Where the ghost cursor is, once there is one.
    ///
    /// Nothing until the box has actually started writing to itself, then everywhere until a
    /// key is touched. It does *not* go out between performances -- the thing you were watching
    /// move vanishing the moment it stopped reads as a bug rather than as a rest -- so it stays
    /// where it finished, as a block, which is where it will set off from next.
    #[must_use]
    pub fn caret(&self) -> Option<usize> {
        self.showing
            .then(|| self.caret.min(self.shown.chars().count()))
    }

    /// Somebody touched a key. Stop where it stands and start the wait over.
    ///
    /// The line does not change. This is called on *every* keystroke, and it used to put a fresh
    /// opener up each time -- so pressing escape, or `i`, or an arrow, rewrote the placeholder
    /// under you for no reason anybody could see. Stopping is not the same as starting again.
    pub fn interrupt(&mut self) {
        self.showing = false;
        self.marked = None;
        self.script.clear();
        self.block = true;
        self.since = Instant::now();
        self.holding = Duration::ZERO;
    }

    /// Put a different line up and start over.
    ///
    /// For when the prompt has actually emptied or filled -- sitting down, or sending something
    /// -- which is when a new line is worth reading and a keystroke is not.
    pub fn restart(&mut self, opener: &str) {
        self.shown = opener.to_owned();
        self.caret = 0;
        self.interrupt();
        self.remember(opener.to_owned());
    }

    /// Play whatever is due, and say whether anything changed.
    ///
    /// Called every frame. When the script runs out it waits `magi.ui.tease_after_ms` and then
    /// writes a new one, changing the line into another from `lines`.
    pub fn advance(&mut self, lines: &[String]) -> bool {
        let after = Duration::from_millis(crate::metric::tease_after_ms());
        if after.is_zero() {
            return false;
        }
        // The duration on an act is how long its *result* is held, not how long to wait before
        // it happens. Those are not the same thing and getting them the wrong way round put the
        // long pause before the selection appeared rather than on the selection -- so the box
        // sat with the cursor doing nothing and then flashed the words it was taking.
        if self.since.elapsed() < self.holding {
            return false;
        }
        let Some(act) = self.script.pop_front() else {
            if self.since.elapsed() < after {
                return false;
            }
            let next = pick(lines, &self.shown, &self.seen).to_owned();
            self.script = perform(&self.shown, &next, self.caret);
            self.showing = !self.script.is_empty();
            self.remember(next);
            self.holding = Duration::ZERO;
            self.since = Instant::now();
            return !self.script.is_empty();
        };
        self.play(&act);
        self.holding = act.over();
        self.since = Instant::now();
        true
    }

    /// Note that a line has been shown, and forget the oldest once too many are held.
    fn remember(&mut self, line: String) {
        if line.is_empty() {
            return;
        }
        self.seen.push_back(line);
        while self.seen.len() > RECALLED {
            self.seen.pop_front();
        }
    }

    /// Carry out one act.
    fn play(&mut self, act: &Act) {
        match act {
            Act::Rest(_) => {}
            Act::Shape { block, .. } => self.block = *block,
            Act::Jump { to, .. } => {
                self.caret = (*to).min(self.shown.chars().count());
                self.marked = None;
            }
            Act::Mark { span, .. } => {
                let end = span.end.min(self.shown.chars().count());
                self.marked = Some(span.start.min(end)..end);
                self.caret = span.start;
            }
            Act::Cut(_) => {
                if let Some(span) = self.marked.take() {
                    let kept: String = self
                        .shown
                        .chars()
                        .enumerate()
                        .filter(|(at, _)| !span.contains(at))
                        .map(|(_, c)| c)
                        .collect();
                    self.shown = kept;
                    self.caret = span.start.min(self.shown.chars().count());
                }
            }
            Act::Put { letter, .. } => {
                let byte = self
                    .shown
                    .char_indices()
                    .nth(self.caret)
                    .map_or(self.shown.len(), |(index, _)| index);
                self.shown.insert(byte, *letter);
                self.caret += 1;
            }
        }
    }
}

/// Where each word of `line` starts, and where the last one ends.
///
/// Word starts are what `w` and `b` land on, so this is the whole of what the ghost cursor is
/// allowed to stop at while it is walking.
#[must_use]
pub fn steps(line: &str) -> Vec<usize> {
    let mut out = vec![0];
    let mut was_space = false;
    for (at, c) in line.chars().enumerate() {
        if was_space && !c.is_whitespace() {
            out.push(at);
        }
        was_space = c.is_whitespace();
    }
    out.push(line.chars().count());
    out.dedup();
    out
}

/// Split a line into its words.
fn words(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

/// Pick a line to change into, preferring one this line can be *edited* into.
///
/// Not at random. The whole point of the engine is the middle edit — walk to a word, take it,
/// write another — and that only happens when two lines share an opening and an ending. Scored,
/// so a pool of unrelated lines still works and simply retypes more of itself, while a pool with
/// families in it finds them without anybody having to group them.
fn pick<'a>(lines: &'a [String], not: &str, seen: &VecDeque<String>) -> &'a str {
    // Anything not shown lately. Without this it picks the closest line to the one on screen,
    // and the closest line to *that* is the one it came from -- so a family of two points at
    // itself and the box swaps between them until somebody types.
    let fresh: Vec<&'a String> = lines
        .iter()
        .filter(|line| line.as_str() != not && !seen.contains(line))
        .collect();
    // Everything has been said recently, which on a short pool happens quickly. Anything but the
    // line already up will do.
    let choices: Vec<&'a String> = if fresh.is_empty() {
        lines.iter().filter(|line| line.as_str() != not).collect()
    } else {
        fresh
    };
    if choices.is_empty() {
        return "";
    }
    // Among those, the one it can make the smallest edit into. The whole point of the engine is
    // the middle edit -- walk to a word, take it, write another -- and that only happens when
    // two lines share an opening and an ending.
    let mine = words(not);
    let best = choices
        .iter()
        .map(|line| kinship(&mine, &words(line)))
        .max()
        .unwrap_or(0);
    let close: Vec<&'a String> = choices
        .into_iter()
        .filter(|line| kinship(&mine, &words(line)) == best)
        .collect();
    // Turned by the clock rather than random: a pool of two picked at random shows the same
    // line twice a quarter of the time, which reads as a stutter rather than as chance.
    let turn = usize::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() % 1_000_000),
    )
    .unwrap_or(0);
    close[turn % close.len()]
}

/// How many words two lines share at the start and the end.
///
/// What a middle edit is measured in: the higher this is, the less has to be retyped and the
/// more the change looks like somebody editing rather than starting again.
fn kinship(from: &[&str], to: &[&str]) -> usize {
    let head = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let tail = from
        .iter()
        .rev()
        .zip(to.iter().rev())
        .take_while(|(a, b)| a == b)
        .take(from.len().min(to.len()).saturating_sub(head))
        .count();
    head + tail
}

/// Which words differ between two lines, as a range of word indices into each.
///
/// The common start and the common end are left alone, so what comes back is the middle that
/// actually changed. That is what makes this an *edit* rather than a retype: `let us build
/// something` into `let us scan something` differs in one word, and the performance is one `cw`.
#[must_use]
pub fn difference(from: &[&str], to: &[&str]) -> (Range<usize>, Range<usize>) {
    let head = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let tail = from
        .iter()
        .rev()
        .zip(to.iter().rev())
        .take_while(|(a, b)| a == b)
        .take(from.len().min(to.len()) - head)
        .count();
    (head..from.len() - tail, head..to.len() - tail)
}

/// Write the script that turns one line into another.
///
/// The shape of it, in order: stand up straight, walk to the word that changes, show what is
/// going, take it, then type. Each of those is one or more [`Act`]s, and adding a flourish means
/// adding one here rather than teaching a state machine a new state.
#[must_use]
pub fn perform(from: &str, to: &str, caret: usize) -> VecDeque<Act> {
    let mut script = VecDeque::new();
    if to.is_empty() || from == to {
        return script;
    }
    let step = Duration::from_millis(crate::metric::tease_step_ms().max(1));
    let look = Duration::from_millis(crate::metric::tease_doubt_ms());
    let (theirs, ours) = (words(from), words(to));
    let (cut, write) = difference(&theirs, &ours);

    // A block, because what follows is a motion and motions happen in normal mode. The ghost
    // says which mode it is miming; the prompt's own mode is untouched.
    script.push_back(Act::Shape {
        block: true,
        over: step * 4,
    });

    // Walk there a word at a time, the way `w` and `b` do. Sliding the cursor character by
    // character would be a different editor.
    let stops = steps(from);
    let target = word_at(from, cut.start);
    for stop in walk(&stops, caret, target) {
        script.push_back(Act::Jump {
            to: stop,
            over: step * 2,
        });
    }

    // `cw`, or `c2w`, or however many words are going. Shown first, and held twice the
    // pause anything else here takes: it is the one moment worth looking at, and a change that
    // deletes before you have read what it deleted is a glitch rather than an edit.
    let span = span_of(from, cut.clone());
    if !span.is_empty() {
        script.push_back(Act::Mark {
            span: span.clone(),
            over: (look * 2).max(step * 12),
        });
        script.push_back(Act::Cut(step * 2));
    }

    // And a bar, because what follows is typing.
    script.push_back(Act::Shape {
        block: false,
        over: step * 2,
    });
    // A cut runs to the start of the next word, so it takes the space after itself with it. The
    // replacement owes that space back, or `build` becoming `scan` leaves `scansomething`.
    let mut replacement = ours[write.clone()].join(" ");
    if !write.is_empty() && cut.end < theirs.len() {
        replacement.push(' ');
    }
    for letter in replacement.chars() {
        script.push_back(Act::Put { letter, over: step });
    }
    // And back to a block, the way `esc` ends an edit. It is also what leaves the ghost in a
    // shape that makes sense while it rests: a bar sitting still for thirty seconds looks like
    // a prompt waiting for you rather than a box that has stopped.
    script.push_back(Act::Shape {
        block: true,
        over: look,
    });
    script
}

/// The character index where word `index` starts.
fn word_at(line: &str, index: usize) -> usize {
    let stops = steps(line);
    *stops.get(index).unwrap_or(stops.last().unwrap_or(&0))
}

/// The characters covered by a range of words, including the space after them.
fn span_of(line: &str, words: Range<usize>) -> Range<usize> {
    if words.is_empty() {
        let at = word_at(line, words.start);
        return at..at;
    }
    let start = word_at(line, words.start);
    let stops = steps(line);
    let end = *stops
        .get(words.end)
        .unwrap_or(stops.last().unwrap_or(&start));
    start..end.max(start)
}

/// The stops between where the cursor is and where it is going, in order.
///
/// Forwards or backwards, one word at a time, so the walk is visible. A cursor that arrives
/// without having travelled has not shown you a motion, it has shown you a jump cut.
fn walk(stops: &[usize], from: usize, to: usize) -> Vec<usize> {
    let at = stops.iter().position(|stop| *stop >= from).unwrap_or(0);
    let want = stops.iter().position(|stop| *stop >= to).unwrap_or(0);
    if at <= want {
        stops[at.min(stops.len())..=want.min(stops.len() - 1)].to_vec()
    } else {
        let mut back: Vec<usize> = stops[want..=at.min(stops.len() - 1)].to_vec();
        back.reverse();
        back
    }
}

/// A performance is a script, and the script is what one line has to do to become another.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_walk_stops_where_w_would() {
        // Word starts, and the end of the line. Nothing between them is a place `w` stops.
        assert_eq!(steps("let us build"), vec![0, 4, 7, 12]);
        assert_eq!(steps(""), vec![0]);
        assert_eq!(steps("one"), vec![0, 3]);
    }

    #[test]
    fn only_the_middle_that_changed_is_touched() {
        // The whole point of editing rather than retyping. Two lines that share their opening
        // and their ending differ in the middle, and that is what a `cw` is for.
        let from = vec!["let", "us", "build", "something"];
        let to = vec!["let", "us", "scan", "something"];
        assert_eq!(difference(&from, &to), (2..3, 2..3));
    }

    #[test]
    fn two_words_going_is_one_change_of_two_words() {
        let from = vec!["let", "us", "build", "a", "thing"];
        let to = vec!["let", "us", "read", "thing"];
        let (cut, write) = difference(&from, &to);
        assert_eq!(cut.len(), 2, "`build a` goes");
        assert_eq!(write.len(), 1, "and `read` arrives");
    }

    #[test]
    fn lines_with_nothing_in_common_are_replaced_whole() {
        let from = vec!["alpha", "beta"];
        let to = vec!["gamma", "delta"];
        assert_eq!(difference(&from, &to), (0..2, 0..2));
    }

    #[test]
    fn the_walk_is_one_word_at_a_time_in_either_direction() {
        // A cursor that arrives without having travelled has shown you a jump cut, not a motion.
        let stops = vec![0, 4, 7, 12];
        assert_eq!(walk(&stops, 0, 7), vec![0, 4, 7]);
        assert_eq!(walk(&stops, 12, 4), vec![12, 7, 4], "and backwards");
    }

    #[test]
    fn a_script_stands_up_walks_shows_cuts_and_types() {
        // The shape of the whole thing, in order. Adding a flourish later means adding an act
        // to this list, not teaching a state machine another state.
        let script = perform("let us build something", "let us scan something", 0);
        let kinds: Vec<&str> = script
            .iter()
            .map(|act| match act {
                Act::Rest(_) => "rest",
                Act::Shape { block: true, .. } => "block",
                Act::Shape { .. } => "bar",
                Act::Jump { .. } => "jump",
                Act::Mark { .. } => "mark",
                Act::Cut(_) => "cut",
                Act::Put { .. } => "put",
            })
            .collect();
        assert_eq!(kinds.first(), Some(&"block"), "a motion needs normal mode");
        assert!(kinds.contains(&"jump"), "it walks there");
        let mark = kinds.iter().position(|k| *k == "mark").expect("it marks");
        let cut = kinds.iter().position(|k| *k == "cut").expect("it cuts");
        assert!(mark < cut, "and shows what is going before it goes");
        let bar = kinds.iter().position(|k| *k == "bar").expect("then a bar");
        assert!(cut < bar, "which is what typing happens in");
        assert!(
            kinds.iter().skip(bar).any(|k| *k == "put"),
            "and then it types"
        );
    }

    #[test]
    fn what_it_shows_is_what_it_takes() {
        // The marked span has to be the words that are going. Marking anything else is telling
        // you one thing and doing another, which is worse than not marking at all.
        let script = perform("let us build something", "let us scan something", 0);
        let marked = script
            .iter()
            .find_map(|act| match act {
                Act::Mark { span, .. } => Some(span.clone()),
                _ => None,
            })
            .expect("it marks");
        let taken: String = "let us build something"
            .chars()
            .skip(marked.start)
            .take(marked.len())
            .collect();
        assert_eq!(taken.trim(), "build");
    }

    #[test]
    fn playing_the_script_makes_the_other_line() {
        // The measurement that matters: whatever the acts are, what comes out the far end is
        // the line it was asked for.
        let mut tease = Tease::new("let us build something");
        for act in perform("let us build something", "let us scan something", 0) {
            tease.play(&act);
        }
        assert_eq!(tease.shown(), "let us scan something");
    }

    #[test]
    fn it_edits_the_middle_rather_than_the_end() {
        let mut tease = Tease::new("open the door slowly");
        for act in perform("open the door slowly", "open the window slowly", 0) {
            tease.play(&act);
        }
        assert_eq!(tease.shown(), "open the window slowly");
    }

    #[test]
    fn a_line_with_nothing_in_common_still_arrives() {
        let mut tease = Tease::new("alpha beta");
        for act in perform("alpha beta", "gamma delta", 0) {
            tease.play(&act);
        }
        assert_eq!(tease.shown(), "gamma delta");
    }

    #[test]
    fn the_cursor_is_a_block_while_it_moves_and_a_bar_while_it_types() {
        // The one thing this is teaching. A ghost that typed with a block cursor would be
        // miming a mode the prompt does not have.
        let mut tease = Tease::new("let us build something");
        let script = perform("let us build something", "let us scan something", 0);
        let mut block_while_jumping = true;
        let mut bar_while_putting = true;
        for act in script {
            tease.play(&act);
            match act {
                Act::Jump { .. } => block_while_jumping &= tease.block,
                Act::Put { .. } => bar_while_putting &= !tease.block,
                _ => {}
            }
        }
        assert!(block_while_jumping, "it moved with a bar cursor");
        assert!(bar_while_putting, "it typed with a block cursor");
    }

    #[test]
    fn a_touched_prompt_stops_it_where_it_stands() {
        // Stops, and does not start again. Every keystroke reaches this, and it used to put a
        // fresh line up each time -- so escape, or `i`, or an arrow rewrote the placeholder
        // under you for no reason anybody could see.
        let mut tease = Tease::new("one two three");
        tease.script = perform("one two three", "one four three", 0);
        tease.play(&Act::Jump {
            to: 4,
            over: Duration::ZERO,
        });
        tease.showing = true;
        tease.interrupt();
        assert_eq!(
            tease.shown(),
            "one two three",
            "the line changed under them"
        );
        assert!(tease.script.is_empty(), "it has stopped");
        assert_eq!(
            tease.caret(),
            None,
            "and the ghost has gone, leaving only yours"
        );
    }

    #[test]
    fn an_emptied_prompt_does_get_a_new_line() {
        // Which is the other half of it: sitting down or sending something is worth a fresh
        // line to read, and a keystroke is not.
        let mut tease = Tease::new("one");
        tease.restart("something else");
        assert_eq!(tease.shown(), "something else");
        assert_eq!(
            tease.caret(),
            None,
            "and it waits again before showing a ghost"
        );
    }

    #[test]
    fn there_is_no_ghost_until_the_box_has_started() {
        // A second cursor on a prompt nobody has left alone yet is a second place to type, and
        // a prompt has one.
        let tease = Tease::new("resting");
        assert_eq!(tease.caret(), None);
    }

    #[test]
    fn the_ghost_stays_between_performances() {
        // Once it is there it is there. The thing you were watching move vanishing the moment
        // it stopped reads as a bug rather than as a rest.
        let mut tease = Tease::new("one two three");
        tease.showing = true;
        tease.play(&Act::Jump {
            to: 4,
            over: Duration::ZERO,
        });
        assert_eq!(
            tease.caret(),
            Some(4),
            "with an empty script and no key touched"
        );
    }

    #[test]
    fn the_ghost_is_never_past_the_end_of_the_line() {
        // It is drawn every frame now, so a caret left over from a longer line would index off
        // the end of a shorter one.
        let mut tease = Tease::new("a much longer line than the next one");
        tease.showing = true;
        tease.caret = 30;
        tease.shown = "short".to_owned();
        assert_eq!(tease.caret(), Some(5));
    }
}
