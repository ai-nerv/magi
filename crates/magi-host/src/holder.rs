//! Driving a surface: reserving its rows, spawning its tenant, pumping frames. A tool runs on a
//! blocking thread inside a turn, the person is behind an async socket, and the tenant is a spawned
//! process — so the reservation goes out as an event, keys come back on a channel, and the blocking
//! receive doubles as the clock. magi reserves; the tenant draws, and what comes back is blitted unread.

use magi_proto::surfacing::{FromSurface, ToSurface};
use magi_proto::tooling::Surface;
use magi_proto::{Cursor, HarnessEvent, ToolCallId};
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

/// How long a surface may hold the screen with nobody touching it.
const PATIENCE: Duration = Duration::from_secs(900);

/// The tick a surface gets when it asked for none. The loop always has a timeout so it can notice
/// the session ended; a surface that wants no ticks is simply not sent one when this elapses.
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
    /// A holder that publishes through `publish` and spawns `program`. It answers nothing until it
    /// is given something that can — see [`Holder::knowing`].
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
    fn hold(
        &self,
        tool: &str,
        surface: &Surface,
        args: &serde_json::Value,
        context: &magi_tools::holding::Context,
    ) -> Option<String> {
        // Nobody is looking, or nobody looking can draw. Reserving rows on a screen that does not
        // exist holds the turn open until the surface times out, on a keypress that never comes.
        if !(self.attached)() || !self.held.on_a_screen() {
            return None;
        }
        // A window too short for any of it is the same case as nobody looking.
        let granted = self.held.grant(surface.rows, surface.place)?;
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("s{n}"));
        let keys = self.held.opening(id.clone())?;

        // Told before the tenant is spawned, so the rows exist by the time the first frame arrives.
        (self.publish)(HarnessEvent::Surfaced {
            cursor: Cursor::ZERO,
            id: id.clone(),
            tool: tool.to_owned(),
            rows: granted,
            about: surface.about.clone(),
            place: surface.place,
        });

        let answered = self
            .spawn(tool, surface, context)
            .and_then(|child| self.pump(child, &id, surface, granted, args, &keys));

        self.held.close(&id);
        (self.publish)(HarnessEvent::Unsurfaced {
            cursor: Cursor::ZERO,
            id,
        });
        answered
    }
}

impl Holder {
    fn spawn(
        &self,
        tool: &str,
        surface: &Surface,
        context: &magi_tools::holding::Context,
    ) -> Option<std::process::Child> {
        let mut command = std::process::Command::new(&self.program);
        context.apply(&mut command);
        command
            .arg("surface")
            // The tenant, where the one that asked handed the screen on to another tool.
            .arg(surface.tenant.as_deref().unwrap_or(tool))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .inspect_err(|why| {
                magi_model::noted!("holder: {} surface {tool}: {why}", self.program);
            })
            .ok()
    }

    /// Exchange frames with the tenant until it is done.
    fn pump(
        &self,
        mut child: std::process::Child,
        id: &ToolCallId,
        surface: &Surface,
        granted: u16,
        args: &serde_json::Value,
        nudges: &std::sync::mpsc::Receiver<Nudge>,
    ) -> Option<String> {
        let mut writing = child.stdin.take()?;
        let mut reading = std::io::BufReader::new(child.stdout.take()?);

        // The height is granted, the width is reported: magi decides how many rows a tool gets,
        // and the width is whatever the window happens to be, measured by the client.
        let opened = ToSurface::Open {
            rows: granted,
            cols: self.held.width(surface.place),
            holds: self.held.reports_holds(),
            args: args.clone(),
        };
        if send(&mut writing, &opened).is_none() {
            let _ = child.kill();
            return None;
        }

        // The first frame is drawn before anything is pressed, so the rows fill as they appear.
        let mut answered = self.take(id, &mut reading, &mut writing);
        let waiting = surface
            .tick
            .map_or(IDLE, |ms| Duration::from_millis(ms.into()));
        let began = std::time::Instant::now();

        while answered.is_none() {
            if began.elapsed() > PATIENCE {
                break;
            }
            // The blocking receive is also the clock: a tick every time nobody has pressed anything.
            let frame = match nudges.recv_timeout(waiting) {
                Ok(Nudge::Key(key, state)) => ToSurface::Key { key, state },
                Ok(Nudge::Pointer(kind, button, row, col)) => ToSurface::Mouse {
                    kind,
                    button,
                    row,
                    col,
                },
                // The room changed, so the grant is made again out of the room there is now. It
                // never grows past what the tool asked for, and it shrinks to nothing, because rows
                // below the fold can be neither seen nor aimed at.
                Ok(Nudge::Room(cols, holds)) => ToSurface::Resize {
                    rows: self
                        .held
                        .grant(surface.rows, surface.place)
                        .unwrap_or_default(),
                    cols: match surface.place {
                        magi_proto::tooling::Place::Float => self.held.width(surface.place),
                        magi_proto::tooling::Place::Prompt => cols,
                    },
                    holds,
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if surface.tick.is_some() => {
                    ToSurface::Tick
                }
                // Wakes only to notice the session is still there; nothing is sent, so a picker is
                // not redrawn four times a second.
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                // The channel is gone, which means the session dropped this surface.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if send(&mut writing, &frame).is_none() {
                break;
            }
            answered = self.take(id, &mut reading, &mut writing);
        }

        // Told rather than killed, then killed, so a tenant that ignores it does not outlive its rows.
        let _ = send(&mut writing, &ToSurface::Close);
        drop(writing);
        let _ = child.kill();
        let _ = child.wait();
        answered
    }

    /// Read frames until the surface says it is done, publishing each one it drew. `None` while it
    /// is still drawing — the caller sends the next event and asks again.
    fn take<R: BufRead, W: Write>(
        &self,
        id: &ToolCallId,
        reading: &mut R,
        writing: &mut W,
    ) -> Option<String> {
        // A question is not the tenant's turn, so reading stops at the first thing that is not a
        // frame rather than leaving the surface a frame behind for the rest of its life.
        loop {
            let mut line = String::new();
            if reading.read_line(&mut line).ok()? == 0 {
                // The tenant closed its output; holding the rows would leave a hole nothing closes.
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
                        // Told rather than dropped: silence and a refusal look identical from
                        // inside a tenant, right up until it is still waiting.
                        None => magi_proto::wondering::Answered::Refused {
                            because: format!("this magi has no `{wonder}` to ask about"),
                        },
                    };
                    send(writing, &ToSurface::Answer { wondered, answered })?;
                }
                // Skipped rather than fatal, so the surface survives a protocol that grew.
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
    // Flushed every frame, or a game's input arrives in batches and it looks frozen and then jumps.
    writing.flush().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_surface_receives_coordinator_settings_not_tool_arguments() {
        use magi_tools::holding::{Context, Holds};
        use std::os::unix::fs::PermissionsExt;
        let dir = magi_model::scratch::Scratch::new("magi-holder", "policy");
        let script = dir.join("tenant");
        std::fs::write(&script, concat!(
            "#!/bin/sh\nread -r opening\n",
            "printf '%s\\n' \"$CASPER_JAIL\" \"$MAGI_TOOLS_CONFIGURE\" \"$CASPER_CONFIGURE\" \"$PWD\" > report\n",
            "printf '%s\\n' '{\"event\":\"done\",\"answered\":\"ok\"}'\n",
        )).expect("tenant script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .expect("executable");
        let (_held, mut holder, _) = on_a_screen_of(8);
        holder.program = script.display().to_string();
        let surface = Surface {
            rows: 4,
            about: "probe".into(),
            tick: None,
            place: magi_proto::tooling::Place::Prompt,
            tenant: Some("probe".into()),
        };
        for jail in [None, Some(r#"{"reach":false,"write":[]}"#.to_owned())] {
            let context = Context {
                configure: "trusted-config".into(),
                jail,
                cwd: Some(dir.to_path_buf()),
            };
            // Spawning a script this process has just written races every other thread's open
            // file descriptors across the fork, and loses with ETXTBSY. Bounded, so a tenant
            // that genuinely never answers still fails the test.
            let forged = serde_json::json!({
                "CASPER_JAIL": "", "cwd": "/", "configure": "forged", "reach": true,
            });
            let mut held = None;
            for _ in 0..20 {
                held = holder.hold("probe", &surface, &forged, &context);
                if held.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert_eq!(held.as_deref(), Some("ok"));
            let report = std::fs::read_to_string(dir.join("report")).expect("spawn report");
            assert_eq!(
                report,
                format!(
                    "{}\ntrusted-config\ntrusted-config\n{}\n",
                    context.jail.as_deref().unwrap_or(""),
                    dir.display()
                )
            );
        }
    }

    #[test]
    fn nobody_attached_reserves_nothing() {
        use magi_tools::holding::Holds;
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
            place: magi_proto::tooling::Place::Prompt,
            tenant: None,
        };
        assert_eq!(
            holder.hold(
                "dino",
                &surface,
                &serde_json::Value::Null,
                &magi_tools::holding::Context::default()
            ),
            None
        );
    }

    /// A holder on a screen with `room` rows, and everything it published. The [`Drawing`] is
    /// leaked deliberately: it stands for a UI attached for as long as the test runs.
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
            // Nothing to spawn, so the surface ends the moment it opens; the reservation is first.
            "not-a-program-anybody-has",
        );
        (held, holder, seen)
    }

    #[test]
    fn a_tenant_is_given_the_rows_there_are_not_the_rows_it_asked_for() {
        use magi_tools::holding::Holds;
        // A tool asking for eight on a window with three used to be granted eight.
        let (_held, holder, seen) = on_a_screen_of(3);
        let surface = Surface {
            rows: 8,
            about: "a game".to_owned(),
            tick: None,
            place: magi_proto::tooling::Place::Prompt,
            tenant: None,
        };
        let _ = holder.hold(
            "dino",
            &surface,
            &serde_json::Value::Null,
            &magi_tools::holding::Context::default(),
        );
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
        let (_held, holder, seen) = on_a_screen_of(0);
        let surface = Surface {
            rows: 8,
            about: "a game".to_owned(),
            tick: None,
            place: magi_proto::tooling::Place::Prompt,
            tenant: None,
        };
        assert_eq!(
            holder.hold(
                "dino",
                &surface,
                &serde_json::Value::Null,
                &magi_tools::holding::Context::default()
            ),
            None
        );
        assert!(seen.lock().expect("nothing panicked").is_empty());
    }

    #[test]
    fn a_tenant_that_closes_its_output_ends_the_surface() {
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
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(|_| {}),
            Box::new(|| true),
            "casper",
        );
        let mut odd = std::io::BufReader::new(&b"{\"event\":\"something_new\"}\n"[..]);
        assert_eq!(
            holder.take(&ToolCallId::new("s0"), &mut odd, &mut Vec::new()),
            None
        );
    }

    #[test]
    fn a_question_is_answered_without_costing_the_surface_its_frame() {
        // The whole ask-back channel in one read: the frame the tenant was already sending arrives.
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
            r#"{"event":"ask","wondered":3,"wonder":"session"}"#,
            "\n",
            r#"{"event":"draw","lines":[[{"role":"text","text":"hi"}]]}"#,
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
        // A tenant is told which of its questions went unanswered rather than left waiting.
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(|_| {}),
            Box::new(|| true),
            "casper",
        );
        let asked = concat!(
            r#"{"event":"ask","wondered":1,"wonder":"siblings"}"#,
            "\n",
            r#"{"event":"done","answered":"once"}"#,
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
        // The rows go to the screen and the answer comes back here.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let holder = Holder::new(
            Arc::new(Holding::new()),
            Box::new(move |event| kept.lock().expect("held").push(event)),
            Box::new(|| true),
            "casper",
        );
        let drew = br#"{"event":"draw","lines":[[{"role":"text","text":"hi"}]]}"#;
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
