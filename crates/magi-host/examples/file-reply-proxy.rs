use serde_json::{Value, json};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const CAP: usize = magi_ipc::family::MAX_FRAME_BYTES;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn line(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for _ in 0..1024 {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        out.extend(byte);
        if byte == [b'\n'] {
            return Ok(out);
        }
    }
    Err("fixture announcement exceeded its bound".into())
}

fn frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > CAP {
        return Err("fixture frame exceeded its bound".into());
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn send(stream: &mut UnixStream, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > CAP {
        return Err("invalid fixture frame".into());
    }
    stream.write_all(&u32::try_from(bytes.len())?.to_be_bytes())?;
    stream.write_all(bytes)?;
    Ok(())
}

fn record(root: &Path, bytes: &[u8]) -> Result<bool> {
    let reply: Value = serde_json::from_slice(bytes)?;
    if reply.get("family").and_then(Value::as_u64) != Some(1)
        || reply.get("n").and_then(Value::as_u64) != Some(1)
        || reply.get("ok").and_then(Value::as_bool) != Some(true)
    {
        return Err("actual Casper did not return one supported result".into());
    }
    let result = reply
        .get("result")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() == 1)
        .and_then(|rows| rows.first())
        .ok_or("actual Casper result missing")?;
    let mutation = result.get("mutation");
    let lost = mutation.is_some() && root.join("drop-next").is_file();
    let event = json!({"mutation_id":mutation.and_then(|value|value.get("id")),
        "outcome":mutation.and_then(|value|value.get("outcome")),"dropped":lost});
    let fd = rustix::fs::open(
        root.join("dispatch-counts.json"),
        rustix::fs::OFlags::RDWR
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    let metadata = rustix::fs::fstat(&fd)?;
    if metadata.st_uid != rustix::process::geteuid().as_raw()
        || metadata.st_nlink != 1
        || rustix::fs::FileType::from_raw_mode(metadata.st_mode)
            != rustix::fs::FileType::RegularFile
        || metadata.st_mode & 0o7777 != 0o600
        || metadata.st_size > 64 * 1024
    {
        return Err("fixture event log is unsafe or full".into());
    }
    let mut log = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    Read::take(&mut log, 64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err("fixture counters exceeded their bound".into());
    }
    let mut counts: Vec<Value> = if bytes.is_empty() {
        Vec::new()
    } else {
        serde_json::from_slice(&bytes)?
    };
    if counts.len() >= 256 {
        return Err("fixture dispatch capacity exhausted".into());
    }
    counts.push(event);
    let encoded = serde_json::to_vec(&counts)?;
    if encoded.len() > 64 * 1024 {
        return Err("fixture counters exceeded their bound".into());
    }
    log.seek(SeekFrom::Start(0))?;
    log.write_all(&encoded)?;
    log.set_len(encoded.len() as u64)?;
    if lost {
        std::fs::remove_file(root.join("drop-next"))?;
    }
    Ok(lost)
}

fn command(program: &str, args: &[String], root: &Path) -> Result<()> {
    let mut child = Owned(
        Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?,
    );
    let mut out = BufReader::new(child.0.stdout.take().ok_or("actual stdout missing")?);
    if args.iter().any(|arg| arg == "--file-admission") {
        let header = line(&mut out)?;
        let event: Value = serde_json::from_slice(&header)?;
        if event.get("event").and_then(Value::as_str) != Some("file-admission")
            || event.get("revision_contract").and_then(Value::as_u64) != Some(1)
        {
            return Err("actual command did not admit native files".into());
        }
        std::io::stdout().write_all(&header)?;
        std::io::stdout().flush()?;
    }
    let mut bytes = Vec::new();
    out.take((CAP + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > CAP || !child.0.wait()?.success() {
        return Err("actual command failed or overflowed".into());
    }
    if !record(root, &bytes)? {
        std::io::stdout().write_all(&bytes)?;
    }
    Ok(())
}

fn socket(program: &str, args: &[String], root: &Path) -> Result<()> {
    let at = args
        .iter()
        .position(|arg| arg == "--at")
        .and_then(|index| args.get(index + 1))
        .ok_or("fixture needs --at")?;
    let public = PathBuf::from(at);
    let upstream = public.with_file_name(format!("up-{}.sock", std::process::id()));
    let mut actual = args.to_vec();
    let index = actual
        .iter()
        .position(|arg| arg == "--at")
        .ok_or("missing actual socket")?;
    *actual.get_mut(index + 1).ok_or("missing actual path")? =
        upstream.to_str().ok_or("socket path is not UTF-8")?.into();
    let key = line(&mut std::io::stdin().lock())?;
    if key.len() != 65 {
        return Err("fixture key has the wrong bound".into());
    }
    let mut child = Owned(
        Command::new(program)
            .args(actual)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?,
    );
    child
        .0
        .stdin
        .take()
        .ok_or("actual stdin missing")?
        .write_all(&key)?;
    let out = child.0.stdout.take().ok_or("actual announcement missing")?;
    let (send_event, receive) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(out);
        let _ = send_event.send(line(&mut reader));
    });
    let announcement = receive.recv_timeout(Duration::from_secs(10))??;
    let mut event: Value = serde_json::from_slice(&announcement)?;
    if event.get("event").and_then(Value::as_str) != Some("listening")
        || event.get("at").and_then(Value::as_str) != upstream.to_str()
        || event.get("file_admission").and_then(Value::as_u64) != Some(1)
    {
        return Err("actual daemon did not admit its native socket".into());
    }
    std::fs::write(root.join("actual-announcement.json"), &announcement)?;
    let listener = UnixListener::bind(&public)?;
    event
        .as_object_mut()
        .ok_or("actual announcement is not an object")?
        .insert("at".into(), json!(public));
    println!("{event}");
    std::io::stdout().flush()?;
    for _ in 0..256 {
        let (mut client, _) = listener.accept()?;
        if !magi_ipc::peer_is_us(&client) {
            return Err("foreign fixture client".into());
        }
        client.set_read_timeout(Some(Duration::from_secs(10)))?;
        client.set_write_timeout(Some(Duration::from_secs(10)))?;
        let request = frame(&mut client)?;
        let mut actual = UnixStream::connect(&upstream)?;
        if !magi_ipc::peer_is_us(&actual) {
            return Err("foreign actual daemon".into());
        }
        actual.set_read_timeout(Some(Duration::from_secs(10)))?;
        actual.set_write_timeout(Some(Duration::from_secs(10)))?;
        send(&mut actual, &request)?;
        let reply = frame(&mut actual)?;
        if record(root, &reply)? {
            client.shutdown(std::net::Shutdown::Both)?;
        } else {
            send(&mut client, &reply)?;
        }
    }
    Err("fixture request capacity exhausted".into())
}

fn main() -> Result<()> {
    let parent = rustix::process::getppid();
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::Term))?;
    if rustix::process::getppid() != parent {
        return Err("fixture caller exited".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    let program = args.first().ok_or("fixture needs a selected Casper")?;
    let root = Path::new(args.get(1).ok_or("fixture needs a private root")?);
    let forwarded = args.get(2..).ok_or("fixture needs a verb")?;
    let result = match forwarded.first().map(String::as_str) {
        Some("run") => command(program, forwarded, root),
        Some("serve") => socket(program, forwarded, root),
        _ => Err("unsupported fixture verb".into()),
    };
    if let Err(error) = &result {
        let _ = std::fs::write(root.join("proxy-error.txt"), error.to_string());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_frames_refuse_invalid_bounds_and_preserve_exact_bytes() -> Result<()> {
        let (mut sender, mut receiver) = UnixStream::pair()?;
        let bytes = br#"{"key":"not-logged","args":[{"tool":"read"}]}"#;
        send(&mut sender, bytes)?;
        assert_eq!(frame(&mut receiver)?, bytes);
        sender.write_all(&u32::MAX.to_be_bytes())?;
        assert!(frame(&mut receiver).is_err());
        assert!(send(&mut sender, &[]).is_err());
        Ok(())
    }
}
