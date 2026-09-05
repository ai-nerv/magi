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

use magi_proto::surfacing::{FromSurface, ToSurface};
use magi_proto::tooling::Surface;
use magi_proto::{Cursor, HarnessEvent, ToolCallId};
use std::io::{BufRead, Write};
use std::sync::Arc;
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

/// Which surfaces are open, and what reaches them.
mod holding;
pub use holding::{Drawing, Holding, Nudge};

/// Gives a tool rows by publishing, and drives its tenant over a spawn.
pub struct Holder {
    held: Arc<Holding>,
    publish: Box<dyn Fn(HarnessEvent) + Send + Sync>,
    attached: Box<dyn Fn() -> bool + Send + Sync>,
    knows: Arc<dyn magi_tools::holding::Answers>,
    program: String,
    next: std::sync::atomic::AtomicU64,
}

impl Holder {
    /// A holder that publishes through `publish` and spawns `program`.
    ///
    /// It answers nothing until it is given something that can — see [`Holder::knowing`]. A
    /// holder built for a screen and nothing else is the ordinary case in a test, and one that
    /// invented answers there would be one whose tests prove nothing.
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
            knows: Arc::new(magi_tools::holding::Incurious),
            program: program.to_owned(),
            next: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Let its surfaces ask `knows` about the session.
    #[must_use]
    pub fn knowing(mut self, knows: Arc<dyn magi_tools::holding::Answers>) -> Self {
        self.knows = knows;
        self
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
        let mut answered = self.take(id, &mut reading, &mut writing);
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
            answered = self.take(id, &mut reading, &mut writing);
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
    fn take<R: BufRead, W: Write>(
        &self,
        id: &ToolCallId,
        reading: &mut R,
        writing: &mut W,
    ) -> Option<String> {
        // A question is not the tenant's turn. It asks, magi answers, and the frame it was going
        // to send is still coming — so reading stops at the first thing that is not one, rather
        // than treating an `Ask` as the frame this cycle was waiting for and leaving the surface
        // a frame behind for the rest of its life.
        loop {
            let mut line = String::new();
            if reading.read_line(&mut line).ok()? == 0 {
                // The tenant closed its output: it exited, or it was killed. Either way the rows
                // will not be filled again, and holding them would leave a hole nothing can close.
                return Some(String::new());
            }
            match serde_json::from_str(line.trim()) {
                Ok(FromSurface::Draw { lines, cursor }) => {
                    (self.publish)(HarnessEvent::Drew {
                        id: id.clone(),
                        lines,
                        cursor,
                    });
                    return None;
                }
                Ok(FromSurface::Done { answered }) => return Some(answered),
                Ok(FromSurface::Ask {
                    wondered,
                    wonder,
                    args,
                }) => {
                    let answered = match magi_proto::wondering::Wonder::named(&wonder) {
                        Some(wonder) => self.knows.answer(wonder, &args),
                        // A newer casper asking something with no name here. Told rather than
                        // dropped: silence and a refusal look identical from inside a tenant,
                        // right up until it is still waiting.
                        None => magi_proto::wondering::Answered::Refused {
                            because: format!("this magi has no `{wonder}` to ask about"),
                        },
                    };
                    send(writing, &ToSurface::Answer { wondered, answered })?;
                }
                // A frame this build cannot read is a newer casper saying something with no name
                // here. Skipped rather than fatal, so the surface survives a protocol that grew.
                Err(_) => return None,
            }
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
    use std::sync::Mutex;

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
    ///
    /// The screen is counted with a [`Drawing`] that is deliberately leaked: it stands for a UI
    /// that is attached for as long as the test runs, and one dropped at the end of this function
    /// would be a client that detached before the surface opened.
    fn on_a_screen_of(room: u16) -> (Arc<Holding>, Holder, Arc<Mutex<Vec<HarnessEvent>>>) {
        let held = Arc::new(Holding::new());
        std::mem::forget(Drawing::attach(&held, true));
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
            holder.take(&ToolCallId::new("s0"), &mut empty, &mut Vec::new()),
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
        assert_eq!(
            holder.take(&ToolCallId::new("s0"), &mut odd, &mut Vec::new()),
            None
        );
    }

    #[test]
    fn a_question_is_answered_without_costing_the_surface_its_frame() {
        // The whole ask-back channel in one read. A tenant asks, magi answers, and the frame the
        // tenant was already sending still arrives — reading the question as though it were that
        // frame would leave the surface one behind for the rest of its life.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(move |event| kept.lock().expect("held").push(event)),
            Box::new(|| true),
            "casper",
        )
        .knowing(Arc::new(crate::knowing::Knows::of(
            &magi_proto::SessionId::new("s-7"),
            "/tmp/project",
        )));

        let said = concat!(
            r#"{"from":"ask","wondered":3,"wonder":"session"}"#,
            "\n",
            r#"{"from":"draw","lines":[[{"role":"text","text":"hi"}]]}"#,
            "\n"
        );
        let mut both = std::io::BufReader::new(said.as_bytes());
        let mut answered = Vec::new();
        assert_eq!(
            holder.take(&ToolCallId::new("s0"), &mut both, &mut answered),
            None,
            "the draw was the frame, not the question"
        );
        let back: ToSurface =
            serde_json::from_slice(answered.trim_ascii_end()).expect("an answer was written");
        let ToSurface::Answer {
            wondered,
            answered: magi_proto::wondering::Answered::Told { said },
        } = back
        else {
            panic!("a session always knows which one it is: {back:?}");
        };
        assert_eq!(wondered, magi_proto::wondering::Wondered(3));
        assert_eq!(said["id"], "s-7");
        assert!(matches!(
            seen.lock().expect("held").first(),
            Some(HarnessEvent::Drew { .. })
        ));
    }

    #[test]
    fn a_verb_this_magi_does_not_know_is_refused_by_name() {
        // Silence and a refusal look the same from inside a tenant, right up until it is still
        // waiting. A newer casper asking something with no name here is told so, and told which
        // of its questions went unanswered.
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(|_| {}),
            Box::new(|| true),
            "casper",
        );
        let asked = concat!(
            r#"{"from":"ask","wondered":1,"wonder":"siblings"}"#,
            "\n",
            r#"{"from":"done","answered":"once"}"#,
            "\n"
        );
        let mut both = std::io::BufReader::new(asked.as_bytes());
        let mut answered = Vec::new();
        assert_eq!(
            holder.take(&ToolCallId::new("s0"), &mut both, &mut answered),
            Some("once".to_owned())
        );
        let back: ToSurface =
            serde_json::from_slice(answered.trim_ascii_end()).expect("an answer was written");
        let ToSurface::Answer {
            answered: magi_proto::wondering::Answered::Refused { because },
            ..
        } = back
        else {
            panic!("there is no such verb here: {back:?}");
        };
        assert!(because.contains("siblings"), "{because}");
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
        assert_eq!(
            holder.take(&ToolCallId::new("s0"), &mut one, &mut Vec::new()),
            None
        );
        assert!(matches!(
            seen.lock().expect("held").first(),
            Some(HarnessEvent::Drew { .. })
        ));
    }
}
