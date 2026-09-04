//! Which surfaces are open, and what reaches them.
//!
//! The registry, split from the thing that drives a tenant. What is here is shared and touched
//! from everywhere — a client attaching, a key arriving on the socket, a window resizing — while
//! [`super::Holder`] is one surface's own spawn and its loop. They are different lifetimes and
//! different callers, and the seam is exactly where the mutex is.

use magi_proto::ToolCallId;
use std::sync::Mutex;

/// Something for an open surface to wake up about.
///
/// Two things reach a tenant from outside its own clock, and both arrive on one channel because
/// the loop that reads them is the loop that blocks: a second source would need a second thread to
/// wait on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nudge {
    /// A key the person pressed.
    Key(String, magi_proto::surfacing::Held),
    /// The pointer, in the surface's own coordinates.
    ///
    /// Translated before it got here, by the only thing that knows where the rows landed. What
    /// arrives is a row and a column inside the reservation, and anything outside it never
    /// arrives at all.
    Pointer(
        magi_proto::surfacing::Pointed,
        Option<magi_proto::surfacing::Button>,
        u16,
        u16,
    ),
    /// The room under it changed, because the window or the prompt did.
    ///
    /// **Width is the terminal's, height is magi's — but neither is promised once.** The width
    /// travels here, because it is whatever the window happens to be and nothing else knows it.
    /// The height does not: it is a grant, so it is worked out again where grants are made, out of
    /// the room now reported. Otherwise the number would be decided in two places.
    Room(u16, bool),
}

/// Surfaces currently on screen, and what reaches them.
#[derive(Default)]
pub struct Holding {
    typing: Mutex<std::collections::HashMap<ToolCallId, std::sync::mpsc::Sender<Nudge>>>,
    /// How wide the screen is, as the client last reported it.
    ///
    /// Zero until one says. The session has no terminal of its own, and a tenant told a width
    /// nobody measured lays itself out for a screen that is not there.
    cols: std::sync::atomic::AtomicU16,
    /// How many rows a surface could be drawn in, as the client last reported them.
    ///
    /// Meaningless until [`Holding::measured`] is set, because zero is a real answer here — a
    /// window short enough to have no room at all — and one that has to be told apart from nobody
    /// having said yet.
    rows: std::sync::atomic::AtomicU16,
    /// Whether any client has reported its room.
    ///
    /// Before the first [`Holding::sized`] a tool gets what it asked for. It is the only honest
    /// answer: refusing would deny a surface on a screen that has room for it, and granting a
    /// measured zero would deny one on the strength of a number nobody supplied.
    measured: std::sync::atomic::AtomicBool,
    /// How many attached clients can draw rows a tool asks for.
    ///
    /// A count rather than a flag, because a session may be attached to twice: a surface is worth
    /// reserving while at least one client that can draw one is still there.
    screens: std::sync::atomic::AtomicUsize,
    /// Whether the screen can report a key being held.
    ///
    /// The Kitty keyboard protocol. A tenant is told at open, so one that would otherwise wait for
    /// a release knows there is never going to be one here and can behave accordingly rather than
    /// look broken on the terminals that cannot send one.
    holds: std::sync::atomic::AtomicBool,
}

/// One attached client's ability to draw, for as long as it is attached.
///
/// A guard rather than a pair of calls, because the interesting case is the connection that ends
/// without saying so — a UI that was killed — and a decrement somebody has to remember is a
/// decrement that gets skipped exactly then.
pub struct Drawing<'a> {
    held: Option<&'a Holding>,
}

impl<'a> Drawing<'a> {
    /// Count this client, if it draws.
    #[must_use]
    pub fn attach(held: &'a Holding, draws: bool) -> Self {
        if draws {
            held.screens
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Self {
            held: draws.then_some(held),
        }
    }
}

impl Drop for Drawing<'_> {
    fn drop(&mut self) {
        if let Some(held) = self.held {
            held.screens
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl Holding {
    /// Nothing held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver a key to the surface it was meant for.
    ///
    /// A key for a surface nobody is holding is dropped rather than queued: it belonged to rows
    /// that are gone, and delivering it to whatever holds them now would be acting on a keypress
    /// the person aimed somewhere else.
    pub fn keyed(&self, id: &ToolCallId, key: String, state: magi_proto::surfacing::Held) {
        if let Ok(typing) = self.typing.lock()
            && let Some(sender) = typing.get(id)
        {
            let _ = sender.send(Nudge::Key(key, state));
        }
    }

    /// Deliver a pointer event to the surface it landed in.
    ///
    /// Dropped for a surface nobody holds, for the same reason a key is: those rows are gone, and
    /// whatever is there now is not what the person clicked on.
    pub fn moused(
        &self,
        id: &ToolCallId,
        kind: magi_proto::surfacing::Pointed,
        button: Option<magi_proto::surfacing::Button>,
        row: u16,
        col: u16,
    ) {
        if let Ok(typing) = self.typing.lock()
            && let Some(sender) = typing.get(id)
        {
            let _ = sender.send(Nudge::Pointer(kind, button, row, col));
        }
    }

    /// Register a surface and hand back the end its nudges arrive on.
    pub(super) fn opening(&self, id: ToolCallId) -> Option<std::sync::mpsc::Receiver<Nudge>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.typing.lock().ok()?.insert(id, sender);
        Some(receiver)
    }

    /// Forget one, so its rows are not held for a session that has moved on.
    pub(super) fn close(&self, id: &ToolCallId) {
        if let Ok(mut typing) = self.typing.lock() {
            typing.remove(id);
        }
    }

    /// Whether anybody attached can draw rows a tool asks for.
    #[must_use]
    pub fn on_a_screen(&self) -> bool {
        self.screens.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    /// Whether the screen can report a key being held.
    #[must_use]
    pub fn reports_holds(&self) -> bool {
        self.holds.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Note how much room the screen has and what its keyboard can say, and tell anything drawing
    /// on it.
    ///
    /// Told rather than left to be read: a tenant is asleep between frames, and one that only
    /// learned the width when it next happened to wake would draw at the old one until then.
    pub fn sized(&self, rows: Option<u16>, cols: u16, holds: bool) {
        // **Any of the three can be news.** The width and the room change when the window does;
        // what the keyboard can say changes the first time a repeat or a release arrives, which may
        // be long after a surface opened. Waking only on the width would leave a game that had just
        // been proved able to read a hold still offering the control it had at open.
        let grew = self.cols.swap(cols, std::sync::atomic::Ordering::Relaxed) != cols;
        // Both swaps, then the question. Written as one `||` the second never ran once the first
        // was true, so the very first report — which always changes the room — was the one that
        // left `measured` unset, and every grant after it was made as though nobody had looked.
        let room = rows.is_some_and(|rows| {
            let moved = self.rows.swap(rows, std::sync::atomic::Ordering::Relaxed) != rows;
            let first = !self
                .measured
                .swap(true, std::sync::atomic::Ordering::Relaxed);
            moved || first
        });
        let learned = !self.holds.swap(holds, std::sync::atomic::Ordering::Relaxed) && holds;
        if !grew && !room && !learned {
            return;
        }
        if let Ok(typing) = self.typing.lock() {
            for sender in typing.values() {
                let _ = sender.send(Nudge::Room(cols, holds));
            }
        }
    }

    /// How wide a surface may draw, falling back to a width most terminals have.
    ///
    /// The fallback is for the moment before the first client says: a tenant asked to lay itself
    /// out for zero columns would draw nothing at all.
    #[must_use]
    pub fn across(&self) -> u16 {
        match self.cols.load(std::sync::atomic::Ordering::Relaxed) {
            0 => 80,
            cols => cols,
        }
    }

    /// How many rows there are to grant, or `None` while nobody has measured.
    ///
    /// No fallback, unlike the width. A made-up width costs a tenant one badly wrapped frame; a
    /// made-up height is a tenant laying itself out below the bottom of the screen, believing the
    /// rows are there, and reading keys aimed at the part of it nobody can see.
    #[must_use]
    pub fn down(&self) -> Option<u16> {
        self.measured
            .load(std::sync::atomic::Ordering::Relaxed)
            .then(|| self.rows.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The rows a surface asking for `wanted` may actually have.
    ///
    /// **The grant is the smaller of what was asked and what is there.** A tool asks for the size
    /// it would like to be; only magi knows what else is on the screen, and it is the one that
    /// says. `None` is a screen with no room at all, where the honest answer is that this surface
    /// cannot open rather than that it opened into nothing.
    #[must_use]
    pub fn granting(&self, wanted: u16) -> Option<u16> {
        match self.down() {
            None => Some(wanted),
            Some(0) => None,
            Some(room) => Some(wanted.min(room)),
        }
    }
}

#[cfg(test)]
mod reaching {
    use super::*;
    use magi_proto::surfacing::Held;
    use std::time::Duration;

    #[test]
    fn a_key_for_a_surface_nobody_holds_is_dropped() {
        // Its rows are gone. Delivering it to whatever holds them now would act on a keypress the
        // person aimed somewhere else entirely.
        let held = Holding::new();
        held.keyed(&ToolCallId::new("gone"), "j".to_owned(), Held::Down);
    }

    #[test]
    fn a_key_reaches_the_surface_it_was_meant_for() {
        let held = Holding::new();
        let keys = held.opening(ToolCallId::new("s0")).expect("registered");
        held.keyed(&ToolCallId::new("s0"), "space".to_owned(), Held::Down);
        assert_eq!(
            keys.recv_timeout(Duration::from_secs(1)).ok(),
            Some(Nudge::Key("space".to_owned(), Held::Down))
        );
    }

    #[test]
    fn the_pointer_reaches_the_surface_it_landed_in() {
        use magi_proto::surfacing::{Button, Pointed};
        let held = Holding::new();
        let nudges = held.opening(ToolCallId::new("s0")).expect("registered");
        held.moused(
            &ToolCallId::new("s0"),
            Pointed::Press,
            Some(Button::Left),
            2,
            11,
        );
        assert_eq!(
            nudges.recv_timeout(Duration::from_secs(1)).ok(),
            Some(Nudge::Pointer(Pointed::Press, Some(Button::Left), 2, 11))
        );
    }

    #[test]
    fn a_click_on_rows_nobody_holds_is_dropped() {
        // The same rule a key follows. Those rows are gone, and whatever is drawn there now is
        // not what the person aimed at.
        use magi_proto::surfacing::Pointed;
        let held = Holding::new();
        held.moused(&ToolCallId::new("gone"), Pointed::Press, None, 0, 0);
    }

    #[test]
    fn a_width_that_changed_reaches_everything_drawing() {
        // Told rather than left to be read. A tenant is asleep between frames, and one that only
        // learned the width when it next happened to wake would draw at the old one until then.
        let held = Holding::new();
        let nudges = held.opening(ToolCallId::new("s0")).expect("registered");
        held.sized(Some(20), 120, false);
        assert_eq!(
            nudges.recv_timeout(Duration::from_secs(1)).ok(),
            Some(Nudge::Room(120, false))
        );
        assert_eq!(held.across(), 120);
    }

    #[test]
    fn a_width_that_did_not_change_wakes_nothing() {
        // A redraw sends the size every frame. Forwarding each one would wake a tenant on every
        // keystroke anybody typed anywhere, to tell it something it already knows.
        let held = Holding::new();
        held.sized(Some(20), 120, false);
        let nudges = held.opening(ToolCallId::new("s0")).expect("registered");
        held.sized(Some(20), 120, false);
        assert!(nudges.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn a_width_nobody_has_measured_is_one_a_tenant_can_draw_in() {
        // Before the first client says. A tenant asked to lay itself out for zero columns would
        // draw nothing at all.
        assert_eq!(Holding::new().across(), 80);
    }

    #[test]
    fn the_room_that_shrank_reaches_everything_drawing() {
        // The room changes without the width doing: a prompt that grew a line took a row off
        // every surface on the screen, and a tenant not told is one drawing below the fold.
        let held = Holding::new();
        held.sized(Some(20), 120, false);
        let nudges = held.opening(ToolCallId::new("s0")).expect("registered");
        held.sized(Some(6), 120, false);
        assert_eq!(
            nudges.recv_timeout(Duration::from_secs(1)).ok(),
            Some(Nudge::Room(120, false))
        );
    }

    #[test]
    fn a_grant_is_the_smaller_of_what_was_asked_and_what_is_there() {
        let held = Holding::new();
        // Nobody has measured, so what was asked for is the only number there is.
        assert_eq!(held.granting(8), Some(8));
        held.sized(Some(20), 120, false);
        assert_eq!(held.granting(8), Some(8), "there is room for all of it");
        held.sized(Some(3), 120, false);
        assert_eq!(held.granting(8), Some(3), "there is room for three rows");
        held.sized(Some(0), 120, false);
        assert_eq!(held.granting(8), None, "a screen with no room grants none");
    }

    #[test]
    fn a_closed_surface_stops_taking_keys() {
        let held = Holding::new();
        let keys = held.opening(ToolCallId::new("s0")).expect("registered");
        held.close(&ToolCallId::new("s0"));
        held.keyed(&ToolCallId::new("s0"), "j".to_owned(), Held::Down);
        assert!(keys.recv_timeout(Duration::from_millis(50)).is_err());
    }
}

/// Who can be given rows, and who cannot.
#[cfg(test)]
mod screens {
    use super::*;
    use crate::holder::Holder;
    use magi_proto::tooling::Surface;
    use std::sync::Arc;

    #[test]
    fn a_client_that_cannot_draw_is_not_a_screen() {
        // `magi -p`. Reserving rows for it would hold the turn open until the surface timed out,
        // waiting on a keypress from a terminal that is not there.
        let held = Holding::new();
        let _print = Drawing::attach(&held, false);
        assert!(!held.on_a_screen());
    }

    #[test]
    fn a_ui_is_a_screen_for_as_long_as_it_is_attached() {
        let held = Holding::new();
        {
            let _ui = Drawing::attach(&held, true);
            assert!(held.on_a_screen());
        }
        // Dropped with the connection, so a UI that was killed takes its screen with it rather
        // than leaving the session believing there is still one.
        assert!(!held.on_a_screen());
    }

    #[test]
    fn one_screen_among_several_clients_is_enough() {
        // A session can be attached to twice. A surface is worth reserving while at least one
        // client that can draw it is still there.
        let held = Holding::new();
        let _print = Drawing::attach(&held, false);
        let ui = Drawing::attach(&held, true);
        assert!(held.on_a_screen());
        drop(ui);
        assert!(!held.on_a_screen());
    }

    #[test]
    fn nothing_is_reserved_when_no_screen_can_draw_it() {
        use magi_tools::holding::Holds;
        let held = Arc::new(Holding::new());
        let holder = Holder::new(
            Arc::clone(&held),
            Box::new(|_| {}),
            // Attached — something is listening to events — but nothing that can draw.
            Box::new(|| true),
            "casper",
        );
        let _print = Drawing::attach(&held, false);
        let surface = Surface {
            rows: 8,
            about: "a game".to_owned(),
            tick: Some(60),
        };
        assert_eq!(
            holder.hold("dino", &surface, &serde_json::Value::Null),
            None
        );
    }
}
