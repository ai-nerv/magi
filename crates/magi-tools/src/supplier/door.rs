//! The library door: one `serve` per session settings, reached over its socket. `serve` is handed
//! a key on its standard input when it starts and every call carries it. A call whose request was
//! partly written is never made again.

use crate::Cancel;
use crate::family::{FRAME_CAP, peer_is_us};
use crate::native::Child;
use magi_proto::tooling::{Call, Ran};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How a call over the socket went wrong.
#[derive(Debug)]
pub(super) enum Failed {
    /// Nothing reached the daemon, so the call may be made another way.
    Unsent(String),
    /// Some of the call went out: it may have run, and must not run again.
    Sent(super::failure::Failure),
}

/// How long writing a call, or the rest of a reply once it has begun, may take.
pub(super) const MID_FRAME: Duration = Duration::from_secs(30);

/// How long a call may run before its answer is given up on.
pub(super) const PATIENCE: Duration = Duration::from_secs(60 * 60);

/// How often a wait looks at the cancel flag.
pub(super) const TICK: Duration = Duration::from_millis(100);

/// How long `serve` has to say it is listening.
const ANNOUNCE: Duration = Duration::from_secs(10);

/// One running `serve`: where it listens, the key it was given, and the process.
struct Daemon {
    path: PathBuf,
    key: String,
    child: Child,
    file_admission: bool,
    request_context: bool,
    socket: Option<(u64, u64)>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.socket.is_some() && socket_identity(&self.path) == self.socket {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The daemons this session started, by (program, settings, jail, cwd).
fn daemons() -> &'static Mutex<HashMap<String, Daemon>> {
    static DAEMONS: OnceLock<Mutex<HashMap<String, Daemon>>> = OnceLock::new();
    DAEMONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Socket names are never reused within a process.
static NEXT: AtomicU64 = AtomicU64::new(0);
static ENDING: AtomicBool = AtomicBool::new(false);

fn socket_identity(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let metadata = std::fs::symlink_metadata(path).ok()?;
    (metadata.file_type().is_socket() && metadata.uid() == rustix::process::geteuid().as_raw())
        .then_some((metadata.dev(), metadata.ino()))
}

pub(super) fn shutdown() {
    let retired = {
        let mut running = daemons()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ENDING.store(true, Ordering::Release);
        std::mem::take(&mut *running)
    };
    drop(retired);
}

/// The daemon a `context` is served by: one per program, settings, jail, cwd and environment,
/// hashed so no environment value is held in the key.
fn wanted(program: &str, context: &crate::holding::Context) -> Option<String> {
    use sha2::{Digest, Sha256};
    let said = serde_json::to_vec(&(
        program,
        &context.configure,
        &context.jail,
        &context.cwd,
        &context.environ,
    ))
    .ok()?;
    Some(format!("{:x}", Sha256::digest(said)))
}

/// Put a daemon already running at `path` in front of `context`, as if it had been started here.
#[cfg(test)]
pub(super) fn adopt(
    program: &str,
    context: &crate::holding::Context,
    path: PathBuf,
    child: std::process::Child,
) -> Option<String> {
    let key = magi_ipc::random_hex(32).ok()?;
    let daemon = Daemon {
        path,
        key: key.clone(),
        child: child.into(),
        file_admission: false,
        request_context: false,
        socket: None,
    };
    let superseded = daemons()
        .lock()
        .ok()?
        .insert(wanted(program, context)?, daemon);
    drop(superseded);
    Some(key)
}

#[cfg(test)]
pub(super) fn discard(program: &str, context: &crate::holding::Context) {
    if let Some(key) = wanted(program, context)
        && let Ok(mut daemons) = daemons().lock()
    {
        daemons.remove(&key);
    }
}

/// The socket and key of a running `program serve` for `context`, started the first time and
/// reused after. A daemon that has exited, or that a newer one replaced, is killed and reaped.
pub(super) fn serving(
    program: &str,
    context: &crate::holding::Context,
) -> Option<(PathBuf, String, bool, bool)> {
    if ENDING.load(Ordering::Acquire) {
        return None;
    }
    let dir = magi_ipc::runtime_base("casper");
    let wanted = wanted(program, context)?;
    let gone = {
        let mut running = daemons().lock().ok()?;
        if let Some(daemon) = running.get_mut(&wanted)
            && matches!(daemon.child.try_wait(), Ok(None))
        {
            return Some((
                daemon.path.clone(),
                daemon.key.clone(),
                daemon.file_admission,
                daemon.request_context,
            ));
        }
        running.remove(&wanted)
    };
    drop(gone);

    let path = dir.join(format!(
        "magi-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let key = magi_ipc::random_hex(32).ok()?;
    let mut command = std::process::Command::new(program);
    command
        .arg("serve")
        .arg("--at")
        .arg(&path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    context.apply(&mut command);
    let mut daemon = Daemon {
        path: path.clone(),
        key: key.clone(),
        child: spawn_kept(command).ok()?,
        file_admission: false,
        request_context: false,
        socket: None,
    };
    if !handed_key(&mut daemon.child, &key) {
        return None;
    }
    (daemon.file_admission, daemon.request_context) = announced(&mut daemon.child, &path)?;
    daemon.socket = socket_identity(&path);
    let file_admission = daemon.file_admission;
    let request_context = daemon.request_context;
    let superseded = {
        let mut running = daemons().lock().ok()?;
        if ENDING.load(Ordering::Acquire) {
            return None;
        }
        running.insert(wanted, daemon)
    };
    drop(superseded);
    Some((path, key, file_admission, request_context))
}

/// Write the key line on `serve`'s standard input and close it.
fn handed_key(child: &mut Child, key: &str) -> bool {
    child
        .input
        .take()
        .is_some_and(|mut stdin| stdin.write_all(format!("{key}\n").as_bytes()).is_ok())
}

/// Whether `serve` said it is listening within [`ANNOUNCE`]. Read on a thread of its own, which
/// drains the rest of what it prints so it never blocks on a full pipe.
fn announced(child: &mut Child, path: &std::path::Path) -> Option<(bool, bool)> {
    let out = child.stdout.take()?;
    let path = path.to_owned();
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(out);
        let announcement = super::admission::listening_context(&mut reader, &path);
        let _ = said.send(announcement);
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    heard.recv_timeout(ANNOUNCE).ok()?.ok()
}

/// Spawn on one long-lived thread, the one a daemon's parent-death signal is then tied to.
fn spawn_kept(command: std::process::Command) -> std::io::Result<Child> {
    type Asked = (
        std::process::Command,
        std::sync::mpsc::Sender<std::io::Result<Child>>,
    );
    static KEEPER: OnceLock<Mutex<std::sync::mpsc::Sender<Asked>>> = OnceLock::new();
    let keeper = KEEPER.get_or_init(|| {
        let (ask, asked) = std::sync::mpsc::channel::<Asked>();
        std::thread::Builder::new()
            .name("magi-daemons".to_owned())
            .spawn(move || {
                for (command, back) in asked {
                    let _ = back.send(super::launch::spawn(command));
                }
            })
            .ok();
        Mutex::new(ask)
    });
    let (back, answer) = std::sync::mpsc::channel();
    keeper
        .lock()
        .map_err(|_| std::io::Error::other("the daemon keeper is gone"))?
        .send((command, back))
        .map_err(|_| std::io::Error::other("the daemon keeper is gone"))?;
    answer
        .recv()
        .map_err(|_| std::io::Error::other("the daemon keeper is gone"))?
}

/// Hand one call to a running daemon as the `run` verb, with its key, and read the reply.
#[cfg(test)]
pub(super) fn over_socket(
    path: &std::path::Path,
    key: &str,
    call: &Call,
    cancel: &dyn Cancel,
) -> Result<Ran, Failed> {
    over_socket_context(path, key, call, cancel, false)
}

pub(super) fn over_socket_context(
    path: &std::path::Path,
    key: &str,
    call: &Call,
    cancel: &dyn Cancel,
    observed: bool,
) -> Result<Ran, Failed> {
    #[derive(serde::Serialize)]
    struct Request<'a> {
        call: &'static str,
        args: [&'a Call; 1],
        key: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        event_context: Option<super::launch::Context>,
    }
    let request = Request {
        call: "run",
        args: [call],
        key,
        event_context: observed.then(super::launch::context).flatten(),
    };
    let body = serde_json::to_vec(&request)
        .map_err(|why| Failed::Unsent(format!("this call will not encode: {why}")))?;
    let first = exchange(path, &body, cancel)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            Failed::Sent(super::failure::Failure::unknown(
                "the socket answered nothing",
            ))
        })?;
    super::admission::result(first, call)
        .map_err(|why| Failed::Sent(super::failure::Failure::unknown(why)))
}

/// Send one framed call on a connection of its own and read the rows of its reply. The connection
/// goes with the call, whatever came back.
fn exchange(
    path: &std::path::Path,
    body: &[u8],
    cancel: &dyn Cancel,
) -> Result<Vec<serde_json::Value>, Failed> {
    let mut stream = UnixStream::connect(path)
        .map_err(|why| Failed::Unsent(format!("connect {}: {why}", path.display())))?;
    if !peer_is_us(&stream) {
        return Err(Failed::Unsent(format!(
            "{} answered as another user",
            path.display()
        )));
    }
    write_frame(&mut stream, body)?;
    let reply = read_frame(&mut stream, cancel)
        .map_err(|why| Failed::Sent(super::failure::Failure::unknown(why)))?;
    super::rows_known(&reply).map_err(Failed::Sent)
}

/// Write one length-prefixed frame within [`MID_FRAME`]. Refused before a byte goes out when it
/// would not fit the family's cap.
pub(super) fn write_frame(to: &mut UnixStream, body: &[u8]) -> Result<(), Failed> {
    if body.len() > FRAME_CAP {
        return Err(Failed::Unsent(format!(
            "the call is {} bytes, over the {FRAME_CAP} a frame may carry",
            body.len()
        )));
    }
    let len = u32::try_from(body.len()).map_err(|_| Failed::Unsent("frame too long".into()))?;
    let mut frame = len.to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    let _ = to.set_write_timeout(Some(MID_FRAME));
    let mut written = 0;
    while let Some(rest) = frame.get(written..).filter(|rest| !rest.is_empty()) {
        match to.write(rest) {
            Ok(0) => break,
            Ok(n) => written += n,
            Err(why) if why.kind() == std::io::ErrorKind::Interrupted => {}
            Err(why) if written == 0 => return Err(Failed::Unsent(why.to_string())),
            Err(why) => {
                return Err(Failed::Sent(super::failure::Failure::unknown(
                    why.to_string(),
                )));
            }
        }
    }
    match written {
        0 => Err(Failed::Unsent("the socket took nothing".to_owned())),
        n if n < frame.len() => Err(Failed::Sent(super::failure::Failure::unknown(
            "the call was cut off",
        ))),
        _ => Ok(()),
    }
}

/// Read one length-prefixed frame's body. The first byte may take as long as the call runs, up
/// to [`PATIENCE`] and while nobody cancels; the rest must follow within [`MID_FRAME`].
pub(super) fn read_frame(from: &mut UnixStream, cancel: &dyn Cancel) -> Result<Vec<u8>, String> {
    let _ = from.set_read_timeout(Some(TICK));
    let started = Instant::now();
    let mut header = [0_u8; 4];
    let mut begun: Option<Instant> = None;
    let mut have = 0;
    let mut fill = |into: &mut [u8], have: &mut usize, begun: &mut Option<Instant>| {
        while let Some(rest) = into.get_mut(*have..).filter(|rest| !rest.is_empty()) {
            match from.read(rest) {
                Ok(0) => return Err("the socket closed mid-reply".to_owned()),
                Ok(n) => {
                    *have += n;
                    begun.get_or_insert_with(Instant::now);
                }
                Err(why)
                    if matches!(
                        why.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(why) => return Err(why.to_string()),
            }
            if cancel.is_cancelled() {
                return Err("cancelled".to_owned());
            }
            match begun {
                Some(at) if at.elapsed() > MID_FRAME => {
                    return Err(format!("the reply stalled for {}s", MID_FRAME.as_secs()));
                }
                None if started.elapsed() > PATIENCE => {
                    return Err(format!("no answer in {}s", PATIENCE.as_secs()));
                }
                _ => {}
            }
        }
        Ok(())
    };
    fill(&mut header, &mut have, &mut begun)?;
    let want = u32::from_be_bytes(header) as usize;
    if want == 0 || want > FRAME_CAP {
        return Err(format!(
            "the reply declared {want} bytes; a frame carries 1 to {FRAME_CAP}"
        ));
    }
    let mut body = vec![0_u8; want];
    let mut got = 0;
    fill(&mut body, &mut got, &mut begun)?;
    Ok(body)
}

#[cfg(test)]
mod corpus;

#[cfg(test)]
mod context;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Uncancelled;

    pub(super) fn call() -> Call {
        Call {
            tool: "shell".to_owned(),
            args: serde_json::json!({ "command": "true" }),
            cwd: String::new(),
            answered: None,
            revision: Default::default(),
        }
    }

    /// A listener that answers each call with `reply`, after counting it.
    fn serve(
        reply: Vec<u8>,
    ) -> (
        crate::family::FamilyScratch,
        PathBuf,
        std::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        let dir = crate::family::FamilyScratch::new("door").expect("a family scratch");
        let path = dir.join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let (seen, calls) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut head = [0_u8; 4];
                if stream.read_exact(&mut head).is_err() {
                    continue;
                }
                let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
                if stream.read_exact(&mut body).is_err() {
                    continue;
                }
                let _ = seen.send(body);
                let _ = stream.write_all(&reply);
            }
        });
        (dir, path, calls)
    }

    pub(super) fn framed(body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn every_call_carries_the_key_it_was_started_with() {
        let ran = serde_json::json!({ "said": "done", "failed": false });
        let reply = serde_json::json!({ "ok": true, "family": 1, "n": 1, "result": [ran] });
        let (_dir, path, calls) = serve(framed(reply.to_string().as_bytes()));
        let key = magi_ipc::random_hex(32).expect("random");
        assert_eq!(key.len(), 64);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        let ran = over_socket(&path, &key, &call(), &Uncancelled).expect("answered");
        assert_eq!(ran.said, "done");
        let sent: serde_json::Value =
            serde_json::from_slice(&calls.recv().expect("a call")).expect("json");
        assert_eq!(sent["key"], key.as_str());
        assert_eq!(sent["call"], "run");
    }

    #[test]
    fn serve_is_handed_its_key_on_stdin_and_says_it_is_listening() {
        let dir = magi_model::scratch::Scratch::new("magi-door", "key");
        let heard = dir.join("heard");
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "read key; printf %s \"$key\" > '{}'; echo '{{\"event\":\"listening\"}}'; exec sleep 5",
                heard.display()
            ))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = spawn_kept(command).expect("spawned");
        let key = magi_ipc::random_hex(32).expect("random");
        assert!(handed_key(&mut child, &key));
        assert_eq!(
            announced(&mut child, &dir.join("socket")),
            Some((false, false))
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(std::fs::read_to_string(&heard).expect("heard"), key);
    }

    #[test]
    fn a_reply_over_the_old_limit_is_read_and_the_call_runs_once() {
        let said = "x".repeat(2 * 1024 * 1024);
        let ran = serde_json::json!({ "said": said, "failed": false });
        let reply = serde_json::json!({ "ok": true, "n": 1, "result": [ran] });
        let (_dir, path, calls) = serve(framed(reply.to_string().as_bytes()));
        let got = over_socket(&path, "k", &call(), &Uncancelled).expect("answered");
        assert_eq!(got.said.len(), said.len());
        assert!(calls.recv().is_ok());
        assert!(calls.try_recv().is_err(), "the call went out twice");
    }

    #[test]
    fn a_reply_that_breaks_off_is_sent_and_never_unsent() {
        // The daemon read the whole call, then died mid-reply: the call may have run.
        let mut cut = framed(br#"{"ok":true,"n":1,"result":[{"said":"half"#);
        cut.truncate(cut.len() - 5);
        let mut lying = (100_u32).to_be_bytes().to_vec();
        lying.extend_from_slice(&cut[4..]);
        let (_dir, path, _calls) = serve(lying);
        match over_socket(&path, "k", &call(), &Uncancelled) {
            Err(Failed::Sent(_)) => {}
            other => panic!("a half reply must not be retried: {other:?}"),
        }
    }

    #[test]
    fn a_refusal_is_said_rather_than_retried() {
        let reply = serde_json::json!({
            "ok": false, "n": 0, "result": [], "fault": "refused",
            "error": "a call to serve needs the key it was started with",
        });
        let (_dir, path, _calls) = serve(framed(reply.to_string().as_bytes()));
        match over_socket(&path, "wrong", &call(), &Uncancelled) {
            Err(Failed::Sent(why)) => assert!(why.contains("needs the key"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn nothing_listening_is_unsent_and_may_be_spawned_instead() {
        let dir = crate::family::FamilyScratch::new("door-none").expect("a family scratch");
        match over_socket(&dir.join("absent.sock"), "k", &call(), &Uncancelled) {
            Err(Failed::Unsent(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_oversized_call_is_refused_before_a_byte_is_sent() {
        let (a, mut b) = UnixStream::pair().expect("pair");
        let mut a = a;
        let body = vec![b'x'; FRAME_CAP + 1];
        assert!(matches!(write_frame(&mut a, &body), Err(Failed::Unsent(_))));
        drop(a);
        let mut anything = Vec::new();
        b.read_to_end(&mut anything).expect("read");
        assert!(anything.is_empty(), "{} bytes went out", anything.len());
    }

    #[test]
    fn a_declared_length_over_the_cap_is_refused_before_allocating() {
        let (mut a, mut b) = UnixStream::pair().expect("pair");
        a.write_all(&u32::MAX.to_be_bytes()).expect("write");
        let why = read_frame(&mut b, &Uncancelled).expect_err("refused");
        assert!(why.contains("declared"), "{why}");
        a.write_all(&0_u32.to_be_bytes()).expect("write");
        assert!(
            read_frame(&mut b, &Uncancelled).is_err(),
            "zero is malformed"
        );
    }

    #[test]
    fn a_cancelled_wait_for_an_answer_stops() {
        struct Now;
        impl Cancel for Now {
            fn is_cancelled(&self) -> bool {
                true
            }
        }
        let (_a, mut b) = UnixStream::pair().expect("pair");
        let started = Instant::now();
        assert_eq!(
            read_frame(&mut b, &Now).expect_err("cancelled"),
            "cancelled"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
