use super::*;
use crate::events::collection::branch::Branch;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
};

const REQUEST: &str = "MAGI_TEST_OBSERVER_REQUEST";
const ENTRY: &str = "events::collection::worker::tests::observer::runner::entry";
const INPUT_LIMIT: u64 = 65_536;
const REAP_BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Drain,
    Stall,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: u8,
    mode: Mode,
    output: PathBuf,
    timeout_ms: u64,
    args: Vec<String>,
}

impl Request {
    fn validate(&self) -> io::Result<()> {
        let valid = self.schema == 1
            && self.output.is_absolute()
            && self.output.as_os_str().len() <= 4096
            && (1..=60_000).contains(&self.timeout_ms)
            && !self.args.is_empty()
            && self.args.len() <= 64
            && self.args.iter().map(String::len).sum::<usize>() <= 32_768
            && self.args.iter().all(|arg| {
                !arg.contains('\0')
                    && !matches!(arg.as_str(), "--event-child" | "--agent-event-bootstrap")
            });
        if valid {
            Ok(())
        } else {
            Err(io::ErrorKind::InvalidInput.into())
        }
    }

    fn read(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut bytes = Vec::new();
        file.take(INPUT_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > INPUT_LIMIT {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let request: Self =
            serde_json::from_slice(&bytes).map_err(|_| io::ErrorKind::InvalidData)?;
        request.validate()?;
        Ok(request)
    }
}

#[derive(Serialize)]
struct Identity {
    pid: u32,
    start_time: String,
    executable_sha256: String,
}

fn digest(path: &Path) -> io::Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)?;
    let metadata = file.metadata()?;
    let size = metadata.len();
    if !metadata.is_file() || size == 0 || size > 64 * 1024 * 1024 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut hash = Sha256::new();
    let mut input = file.take(size + 1);
    let mut buffer = [0; 65_536];
    let mut total = 0;
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(buffer.get(..n).ok_or(io::ErrorKind::InvalidData)?);
        total += n as u64;
    }
    if total != size {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn identity(child: &Child, selected: &Path, hash: String) -> io::Result<Identity> {
    let pid = child.id();
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let start_time = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
        .ok_or(io::ErrorKind::InvalidData)?
        .to_owned();
    let path = format!("/proc/{pid}/exe");
    // Spawn can return while the child is still replacing its image.
    let began = Instant::now();
    while fs::read_link(&path).ok().as_deref() != Some(selected)
        && began.elapsed() < Duration::from_secs(5)
    {
        thread::sleep(POLL_INTERVAL);
    }
    if fs::read_link(&path)? != selected || digest(Path::new(&path))? != hash {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(Identity {
        pid,
        start_time,
        executable_sha256: hash,
    })
}

fn output(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

fn publish(directory: &Path, name: &str, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| io::ErrorKind::InvalidData)?;
    if bytes.len() > 16_384 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let temporary = directory.join(format!("{name}.next"));
    output(&temporary)?.write_all(&bytes)?;
    fs::rename(temporary, directory.join(name))
}

struct Native(Child, bool);

impl Native {
    fn signal(&mut self, signal: rustix::process::Signal) -> io::Result<()> {
        if self.1 {
            return Ok(());
        }
        let pid = rustix::process::Pid::from_raw(
            i32::try_from(self.0.id()).map_err(|_| io::ErrorKind::InvalidData)?,
        )
        .ok_or(io::ErrorKind::InvalidData)?;
        rustix::process::kill_process_group(pid, signal)?;
        Ok(())
    }

    fn status(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.0.try_wait()?;
        self.1 |= status.is_some();
        Ok(status)
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        if !self.1 {
            let _ = self.signal(rustix::process::Signal::Kill);
            let _ = self.0.wait();
        }
    }
}

struct Reaper(u64, Option<Instant>);

impl Reaper {
    fn new() -> io::Result<Self> {
        rustix::process::set_child_subreaper(Some(rustix::process::Pid::INIT))?;
        Ok(Self(0, None))
    }

    fn deadline(&mut self) -> Instant {
        *self.1.get_or_insert_with(|| Instant::now() + REAP_BUDGET)
    }

    fn poll(&mut self) -> io::Result<bool> {
        for _ in 0..256 {
            match rustix::process::wait(rustix::process::WaitOptions::NOHANG) {
                Ok(Some(_)) => self.0 = self.0.saturating_add(1),
                Ok(None) => return Ok(false),
                Err(rustix::io::Errno::CHILD) => return Ok(true),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        let until = self.deadline();
        while matches!(self.poll(), Ok(false)) && Instant::now() < until {
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn command(selected: &Path, args: &[String]) -> Command {
    let mut command = Command::new(selected);
    command.args(["--event-child", "--agent-event-bootstrap"]);
    if !args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--headless")
    {
        command.arg("--headless");
    }
    command.args(args);
    command
}

async fn observe(request: Request) -> io::Result<bool> {
    use tokio::signal::unix::{SignalKind, signal};
    request.validate()?;
    let selected =
        fs::canonicalize(std::env::var_os("MAGI_TEST_BINARY").ok_or(io::ErrorKind::InvalidInput)?)?;
    let hash = digest(&selected)?;
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut reaper = Reaper::new()?;
    fs::DirBuilder::new().mode(0o700).create(&request.output)?;
    let limit = Duration::from_millis(request.timeout_ms);
    let gate = Gate::new(request.mode == Mode::Stall, limit + Duration::from_secs(10));
    let lineage = Lineage::root(Id::generate()?, Id::generate()?);
    let parent = Lineage::root(
        lineage.run_id.ok_or(io::ErrorKind::InvalidData)?,
        Id::generate()?,
    );
    let (host, sender, control) = Host::pair(gate.worker().registrar(), lineage)?;
    let packet = Branch::new(sender, control, parent)
        .map_err(|_| io::ErrorKind::InvalidData)?
        .prepare(io::stdin())
        .map_err(|_| io::ErrorKind::InvalidData)?;
    let mut admissions = Admissions::default();
    admissions.add(host);
    let mut native = Native(
        command(&selected, &request.args)
            .env_remove(REQUEST)
            .process_group(0)
            .stdin(Stdio::from(packet))
            .stdout(output(&request.output.join("stdout"))?)
            .stderr(output(&request.output.join("stderr"))?)
            .spawn()?,
        false,
    );
    let began = Instant::now();
    let identity = identity(&native.0, &selected, hash)?;
    publish(
        &request.output,
        "ready.json",
        &json!({"schema":1,"mode":request.mode,"native":identity}),
    )?;
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    let mut forwarded = 0_u64;
    let mut timed_out = false;
    let mut stopping = None;
    let status = loop {
        admissions.poll();
        if let Some(status) = native.status()? {
            break status;
        }
        if !timed_out && began.elapsed() >= limit {
            timed_out = true;
            native.signal(rustix::process::Signal::Term)?;
            stopping = Some(Instant::now());
        }
        if stopping.is_some_and(|at: Instant| at.elapsed() >= Duration::from_secs(2)) {
            native.signal(rustix::process::Signal::Kill)?;
        }
        tokio::select! {
            _ = term.recv() => {
                native.signal(rustix::process::Signal::Term)?;
                forwarded = forwarded.saturating_add(1);
                stopping.get_or_insert_with(Instant::now);
            }
            _ = interrupt.recv() => {
                native.signal(rustix::process::Signal::Int)?;
                forwarded = forwarded.saturating_add(1);
                stopping.get_or_insert_with(Instant::now);
            }
            _ = tick.tick() => {}
        }
    };
    let elapsed = began.elapsed().as_millis();
    let reaping = Instant::now();
    let until = reaper.deadline();
    let mut reaped = reaper.poll()?;
    while !reaped && Instant::now() < until {
        admissions.poll();
        tokio::time::sleep(POLL_INTERVAL).await;
        reaped = reaper.poll()?;
    }
    let reap_elapsed = reaping.elapsed().as_millis();
    admissions.poll();
    let before = gate.worker().readout().snapshot();
    let unchanged = digest(&selected)? == identity.executable_sha256;
    gate.resume();
    let drain = Instant::now();
    while drain.elapsed() < Duration::from_secs(1) {
        admissions.poll();
        if admissions.0.is_empty() && gate.worker().readout().snapshot().remote_producers == 0 {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    drop(admissions);
    let summary = gate.finish();
    let passed = reaped
        && !timed_out
        && unchanged
        && !summary.worker_failed
        && !summary.shutdown_overrun
        && (request.mode != Mode::Stall
            || (before.counts.accepted == 0 && before.counts.statuses == 0));
    publish(
        &request.output,
        "summary.json",
        &json!({
            "schema":1,"scope":"evaluation-event-observer","mode":request.mode,
            "status":if passed {"PASS"} else {"FAIL"},"native":identity,
            "child_exit_code":status.code(),"child_signal":status.signal(),"child_reaped":native.1,
            "timed_out":timed_out,"forwarded_signals":forwarded,"elapsed_ms":elapsed,
        "signal_target":"selected-child-process-group","adopted_children_reaped":reaper.0,
        "adopted_children_complete":reaped,
        "reap_limit_ms":REAP_BUDGET.as_millis(),"reap_elapsed_ms":reap_elapsed,
            "before_release":before,"collection":summary,"incomplete":summary.incomplete(),
            "descendant_cleanup":"requires-isolated-world-receipt"
        }),
    )?;
    Ok(passed)
}

#[test]
fn entry() {
    let Some(path) = std::env::var_os(REQUEST) else {
        return;
    };
    let result = (|| {
        let request = Request::read(Path::new(&path))?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(observe(request))
    })();
    if let Err(error) = &result {
        eprintln!("observer fixture error: {:?}", error.kind());
    }
    assert!(matches!(result, Ok(true)), "observer fixture failed");
}

#[cfg(test)]
mod tests;
