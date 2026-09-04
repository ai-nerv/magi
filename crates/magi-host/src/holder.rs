//! Driving a surface: reserving its rows, spawning its tenant, pumping frames.
//!
//! The awkward shape, again. A tool runs on a blocking thread deep inside a turn; the person is on
//! the other end of a socket served by an async loop; and now there is a *third* party, a spawned
//! process that draws. Nothing here is async and nothing here may block the session.
//!
//! So: the reservation goes out as an event, keys come back on a channel, and the tenant is a
//! child process talked to over its pipes. The blocking receive on the key channel doubles as the
//! clock — a tenant that asked for a tick gets one every time nobody has pressed anything for that
//! long, which is one loop rather than a thread and a timer.
//!
//! **magi reserves; the tenant draws.** What comes back is blitted without being read. magi could
//! not tell a permission prompt from a game if it wanted to, and it does not want to.

use magi_proto::tooling::{FromSurface, Surface, ToSurface};
use magi_proto::{Cursor, HarnessEvent, ToolCallId};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a surface may hold the screen with nobody touching it.
///
/// Long, because a game is played for as long as somebody wants to play it. Bounded, because a
/// turn that waited forever on a tenant that stopped drawing is a session nothing can recover.
const PATIENCE: Duration = Duration::from_secs(900);

/// The tick a surface gets when it asked for none.
///
/// It still has to wake up to notice that the session ended, so the loop always has a timeout; a
/// surface that wants no ticks simply is not sent one when this elapses.
const IDLE: Duration = Duration::from_millis(250);

/// Something for an open surface to wake up about.
///
/// Two things reach a tenant from outside its own clock, and both arrive on one channel because
/// the loop that reads them is the loop that blocks: a second source would need a second thread to
/// wait on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nudge {
    /// A key the person pressed.
    Key(String, magi_proto::tooling::Held),
    /// The pointer, in the surface's own coordinates.
    ///
    /// Translated before it got here, by the only thing that knows where the rows landed. What
    /// arrives is a row and a column inside the reservation, and anything outside it never
    /// arrives at all.
    Pointer(
        magi_proto::tooling::Pointed,
        Option<magi_proto::tooling::Button>,
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
    pub fn keyed(&self, id: &ToolCallId, key: String, state: magi_proto::tooling::Held) {
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
        kind: magi_proto::tooling::Pointed,
        button: Option<magi_proto::tooling::Button>,
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
    fn opening(&self, id: ToolCallId) -> Option<std::sync::mpsc::Receiver<Nudge>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.typing.lock().ok()?.insert(id, sender);
        Some(receiver)
    }

    /// Forget one, so its rows are not held for a session that has moved on.
    fn close(&self, id: &ToolCallId) {
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

/// Gives a tool rows by publishing, and drives its tenant over a spawn.
pub struct Holder {
    held: Arc<Holding>,
    publish: Box<dyn Fn(HarnessEvent) + Send + Sync>,
    attached: Box<dyn Fn() -> bool + Send + Sync>,
    program: String,
    next: std::sync::atomic::AtomicU64,
}

impl Holder {
    /// A holder that publishes through `publish` and spawns `program`.
    #[must_use]
    pub fn new(
        held: Arc<Holding>,
        publish: Box<dyn Fn(HarnessEvent) + Send + Sync>,
        attached: Box<dyn Fn() -> bool + Send + Sync>,
        program: &str,
    ) -> Self {
        Self {
            held,
            publish,
            attached,
            program: program.to_owned(),
            next: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl magi_tools::holding::Holds for Holder {
    fn hold(&self, tool: &str, surface: &Surface, args: &serde_json::Value) -> Option<String> {
        // Nobody is looking, or nobody looking can draw. Reserving rows on a screen that does not
        // exist holds the turn open until the surface times out, waiting on a keypress that was
        // never coming — which is exactly what `magi -p` would do.
        if !(self.attached)() || !self.held.on_a_screen() {
            return None;
        }
        // **What it gets, not what it asked for.** A window too short for any of it is the same
        // case as nobody looking: there is nowhere to draw, and reserving rows that are not there
        // would hold the turn open on a keypress aimed at nothing.
        let granted = self.held.granting(surface.rows)?;
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("s{n}"));
        let keys = self.held.opening(id.clone())?;

        // Told before the tenant is spawned, so the rows exist by the time the first frame
        // arrives and a surface never draws into space nothing has made.
        (self.publish)(HarnessEvent::Surfaced {
            cursor: Cursor::ZERO,
            id: id.clone(),
            tool: tool.to_owned(),
            rows: granted,
            about: surface.about.clone(),
        });

        let answered = self.pump(&id, tool, surface, granted, args, &keys);

        self.held.close(&id);
        (self.publish)(HarnessEvent::Unsurfaced {
            cursor: Cursor::ZERO,
            id,
        });
        answered
    }
}

impl Holder {
    /// Spawn the tenant and exchange frames until it is done.
    fn pump(
        &self,
        id: &ToolCallId,
        tool: &str,
        surface: &Surface,
        granted: u16,
        args: &serde_json::Value,
        nudges: &std::sync::mpsc::Receiver<Nudge>,
    ) -> Option<String> {
        let mut child = std::process::Command::new(&self.program)
            .arg("surface")
            .arg(tool)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut writing = child.stdin.take()?;
        let mut reading = std::io::BufReader::new(child.stdout.take()?);

        // **The height is granted, the width is reported.** magi decides how many rows a tool
        // gets, because only magi knows what else is on the screen. It decides nothing about the
        // width: that is whatever the window happens to be, it changes while the surface is open,
        // and it arrives here from the client that measured it.
        let opened = ToSurface::Open {
            rows: granted,
            cols: self.held.across(),
            holds: self.held.reports_holds(),
            args: args.clone(),
        };
        if send(&mut writing, &opened).is_none() {
            let _ = child.kill();
            return None;
        }

        // The first frame is drawn before anything is pressed, so the rows are filled the moment
        // they appear rather than looking empty until somebody types.
        let mut answered = self.take(id, &mut reading);
        let waiting = surface
            .tick
            .map_or(IDLE, |ms| Duration::from_millis(ms.into()));
        let began = std::time::Instant::now();

        while answered.is_none() {
            if began.elapsed() > PATIENCE {
                break;
            }
            // The blocking receive is also the clock. A tenant that asked for a tick gets one
            // every time nobody has pressed anything for that long, which is one loop rather than
            // a thread and a timer that would have to be cancelled.
            let frame = match nudges.recv_timeout(waiting) {
                Ok(Nudge::Key(key, state)) => ToSurface::Key { key, state },
                Ok(Nudge::Pointer(kind, button, row, col)) => ToSurface::Mouse {
                    kind,
                    button,
                    row,
                    col,
                },
                // The room changed, so the grant is made again out of the room there is now. It
                // never grows past what the tool asked for — a surface that swelled to fill a
                // maximised window would push the transcript around every time somebody dragged an
                // edge — and it shrinks, including to nothing, because rows below the fold are
                // rows the person can neither see nor aim at.
                Ok(Nudge::Room(cols, holds)) => ToSurface::Resize {
                    rows: self.held.granting(surface.rows).unwrap_or_default(),
                    cols,
                    holds,
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if surface.tick.is_some() => {
                    ToSurface::Tick
                }
                // A surface that wants no ticks still wakes, only to notice the session is still
                // there. Nothing is sent, so a picker is not redrawn four times a second.
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                // The channel is gone, which means the session dropped this surface.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if send(&mut writing, &frame).is_none() {
                break;
            }
            answered = self.take(id, &mut reading);
        }

        // Told rather than killed, so a tenant holding something can put it down. Killed after,
        // because a tenant that ignores it must not outlive the rows it was drawing into.
        let _ = send(&mut writing, &ToSurface::Close);
        drop(writing);
        let _ = child.kill();
        let _ = child.wait();
        answered
    }

    /// Read frames until the surface says it is done, publishing each one it drew.
    ///
    /// `None` while it is still drawing — the caller sends the next event and asks again.
    fn take<R: BufRead>(&self, id: &ToolCallId, reading: &mut R) -> Option<String> {
        let mut line = String::new();
        if reading.read_line(&mut line).ok()? == 0 {
            // The tenant closed its output: it exited, or it was killed. Either way the rows will
            // not be filled again, and holding them would leave a hole nothing can close.
            return Some(String::new());
        }
        match serde_json::from_str(line.trim()) {
            Ok(FromSurface::Draw { lines, cursor }) => {
                (self.publish)(HarnessEvent::Drew {
                    id: id.clone(),
                    lines,
                    cursor,
                });
                None
            }
            Ok(FromSurface::Done { answered }) => Some(answered),
            // A frame this build cannot read is a newer casper saying something with no name here.
            // Skipped rather than fatal, so the surface survives a protocol that grew.
            Err(_) => None,
        }
    }
}

/// Write one frame, or `None` when the tenant is gone.
fn send<W: Write>(writing: &mut W, frame: &ToSurface) -> Option<()> {
    let line = serde_json::to_string(frame).ok()?;
    writing.write_all(line.as_bytes()).ok()?;
    writing.write_all(b"\n").ok()?;
    // Flushed every frame, or a game's input would arrive in batches and it would look frozen and
    // then jump.
    writing.flush().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::tooling::Held;

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
        use magi_proto::tooling::{Button, Pointed};
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
        use magi_proto::tooling::Pointed;
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

    #[test]
    fn nobody_attached_reserves_nothing() {
        use magi_tools::holding::Holds;
        // A screen that does not exist cannot be reserved on, and holding the turn open until it
        // timed out is what this avoids.
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(|_| {}),
            Box::new(|| false),
            "casper",
        );
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

    /// A holder on a screen with `room` rows, and everything it published.
    fn on_a_screen_of(room: u16) -> (Arc<Holding>, Holder, Arc<Mutex<Vec<HarnessEvent>>>) {
        let held = Arc::new(Holding::new());
        held.screens
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        held.sized(Some(room), 120, false);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let holder = Holder::new(
            Arc::clone(&held),
            Box::new(move |event| kept.lock().expect("nothing panicked").push(event)),
            Box::new(|| true),
            // Nothing to spawn, so the surface ends the moment it opens. What is being read here
            // is the reservation, which is published before the tenant exists.
            "not-a-program-anybody-has",
        );
        (held, holder, seen)
    }

    #[test]
    fn a_tenant_is_given_the_rows_there_are_not_the_rows_it_asked_for() {
        use magi_tools::holding::Holds;
        // The gap this closes: a tool asking for eight on a window with three used to be granted
        // eight, and laid itself out for five rows nobody could see or aim a key at.
        let (_held, holder, seen) = on_a_screen_of(3);
        let surface = Surface {
            rows: 8,
            about: "a game".to_owned(),
            tick: None,
        };
        let _ = holder.hold("dino", &surface, &serde_json::Value::Null);
        let granted = seen
            .lock()
            .expect("nothing panicked")
            .iter()
            .find_map(|event| match event {
                HarnessEvent::Surfaced { rows, .. } => Some(*rows),
                _ => None,
            });
        assert_eq!(granted, Some(3));
    }

    #[test]
    fn a_screen_with_no_room_reserves_nothing() {
        use magi_tools::holding::Holds;
        // The same case as nobody looking. Reserving rows that are not there holds the turn open
        // waiting on a keypress aimed at nothing.
        let (_held, holder, seen) = on_a_screen_of(0);
        let surface = Surface {
            rows: 8,
            about: "a game".to_owned(),
            tick: None,
        };
        assert_eq!(
            holder.hold("dino", &surface, &serde_json::Value::Null),
            None
        );
        assert!(seen.lock().expect("nothing panicked").is_empty());
    }

    #[test]
    fn a_tenant_that_closes_its_output_ends_the_surface() {
        // It exited or was killed. The rows cannot be filled again, so the surface ends rather
        // than waiting on a frame that is never coming.
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(|_| {}),
            Box::new(|| true),
            "casper",
        );
        let mut empty = std::io::BufReader::new(&b""[..]);
        assert_eq!(
            holder.take(&ToolCallId::new("s0"), &mut empty),
            Some(String::new())
        );
    }

    #[test]
    fn a_frame_this_build_cannot_read_is_skipped_rather_than_fatal() {
        // A newer casper saying something with no name here. Ending the surface over it would
        // make every addition to the protocol a breaking one.
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(|_| {}),
            Box::new(|| true),
            "casper",
        );
        let mut odd = std::io::BufReader::new(&b"{\"from\":\"something_new\"}\n"[..]);
        assert_eq!(holder.take(&ToolCallId::new("s0"), &mut odd), None);
    }

    #[test]
    fn what_a_surface_drew_is_published_rather_than_returned() {
        // The rows go to the screen and the *answer* comes back here. A surface that returned its
        // pixels would make the tool thread the renderer.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(move |event| kept.lock().expect("held").push(event)),
            Box::new(|| true),
            "casper",
        );
        let drew = br#"{"from":"draw","lines":[[{"role":"text","text":"hi"}]]}"#;
        let mut one = std::io::BufReader::new(&drew[..]);
        assert_eq!(holder.take(&ToolCallId::new("s0"), &mut one), None);
        assert!(matches!(
            seen.lock().expect("held").first(),
            Some(HarnessEvent::Drew { .. })
        ));
    }
}

/// Who can be given rows, and who cannot.
#[cfg(test)]
mod screens {
    use super::*;

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
