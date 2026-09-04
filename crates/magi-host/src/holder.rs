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

/// Surfaces currently on screen, and the keys going to them.
#[derive(Default)]
pub struct Holding {
    typing: Mutex<std::collections::HashMap<ToolCallId, std::sync::mpsc::Sender<String>>>,
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
    pub fn keyed(&self, id: &ToolCallId, key: String) {
        if let Ok(typing) = self.typing.lock()
            && let Some(sender) = typing.get(id)
        {
            let _ = sender.send(key);
        }
    }

    /// Register a surface and hand back the end its keys arrive on.
    fn opening(&self, id: ToolCallId) -> Option<std::sync::mpsc::Receiver<String>> {
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
        if !(self.attached)() {
            // Nobody is looking, so nobody can play, choose or answer. Reserving rows on a screen
            // that does not exist would hold the turn open until it timed out.
            return None;
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("s{n}"));
        let keys = self.held.opening(id.clone())?;

        // Told before the tenant is spawned, so the rows exist by the time the first frame
        // arrives and a surface never draws into space nothing has made.
        (self.publish)(HarnessEvent::Surfaced {
            cursor: Cursor::ZERO,
            id: id.clone(),
            tool: tool.to_owned(),
            rows: surface.rows,
            about: surface.about.clone(),
        });

        let answered = self.pump(&id, tool, surface, args, &keys);

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
        args: &serde_json::Value,
        keys: &std::sync::mpsc::Receiver<String>,
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

        // Columns are not the harness's to promise: the surface is drawn inside whatever the
        // transcript is wide, and the tenant is clipped to it either way. A generous number here
        // beats a wrong one, because a layout built for eighty is readable at a hundred.
        let opened = ToSurface::Open {
            rows: surface.rows,
            cols: 92,
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
            let frame = match keys.recv_timeout(waiting) {
                Ok(key) => ToSurface::Key { key },
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
            Ok(FromSurface::Draw { lines }) => {
                (self.publish)(HarnessEvent::Drew {
                    id: id.clone(),
                    lines,
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

    #[test]
    fn a_key_for_a_surface_nobody_holds_is_dropped() {
        // Its rows are gone. Delivering it to whatever holds them now would act on a keypress the
        // person aimed somewhere else entirely.
        let held = Holding::new();
        held.keyed(&ToolCallId::new("gone"), "j".to_owned());
    }

    #[test]
    fn a_key_reaches_the_surface_it_was_meant_for() {
        let held = Holding::new();
        let keys = held.opening(ToolCallId::new("s0")).expect("registered");
        held.keyed(&ToolCallId::new("s0"), "space".to_owned());
        assert_eq!(
            keys.recv_timeout(Duration::from_secs(1)).ok(),
            Some("space".to_owned())
        );
    }

    #[test]
    fn a_closed_surface_stops_taking_keys() {
        let held = Holding::new();
        let keys = held.opening(ToolCallId::new("s0")).expect("registered");
        held.close(&ToolCallId::new("s0"));
        held.keyed(&ToolCallId::new("s0"), "j".to_owned());
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
