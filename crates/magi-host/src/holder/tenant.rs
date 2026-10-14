//! A surface's tenant, as the thread driving it sees it: a process reaped whatever happens, and
//! what it writes read on a thread of its own, so the driver waits on a clock and never on it.

use std::io::{BufRead, Read};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The longest one line from a tenant may be; a longer one is dropped whole.
const LINE_MOST: u64 = 4 * 1024 * 1024;

/// How long the driver waits for a tenant's next line before it gives up on this exchange.
const FRAME_PATIENCE: Duration = Duration::from_secs(30);

/// How often a wait looks up to see whether the surface is still open.
const SLICE: Duration = Duration::from_millis(50);

/// A tenant process, killed and reaped when it is dropped.
pub(super) struct Tenant(pub(super) std::process::Child);

impl Drop for Tenant {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Read `pipe` a line at a time on a thread of its own, each line handed over whole.
pub(super) fn listen(pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (lines, heard) = mpsc::channel();
    std::thread::spawn(move || {
        let mut pipe = std::io::BufReader::new(pipe);
        let mut line = Vec::new();
        loop {
            line.clear();
            match pipe.by_ref().take(LINE_MOST).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if line.last() != Some(&b'\n') && line.len() as u64 == LINE_MOST {
                // The rest of an over-long line is read and dropped with it.
                let mut rest = Vec::new();
                if pipe.read_until(b'\n', &mut rest).is_err() {
                    return;
                }
                continue;
            }
            if lines.send(std::mem::take(&mut line)).is_err() {
                return;
            }
        }
    });
    heard
}

/// What a tenant wrote, as a reader: a read waits at most [`FRAME_PATIENCE`], and only while
/// `open` says the surface is still there. The end of its output reads as the end of a stream.
pub(super) struct Fed<'a> {
    pub(super) lines: mpsc::Receiver<Vec<u8>>,
    pub(super) pending: std::io::Cursor<Vec<u8>>,
    pub(super) open: &'a dyn Fn() -> bool,
}

impl Fed<'_> {
    fn refill(&mut self) -> std::io::Result<()> {
        let deadline = Instant::now() + FRAME_PATIENCE;
        loop {
            match self.lines.recv_timeout(SLICE) {
                Ok(line) => {
                    self.pending = std::io::Cursor::new(line);
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout)
                    if !(self.open)() || Instant::now() > deadline =>
                {
                    return Err(std::io::ErrorKind::TimedOut.into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn drained(&self) -> bool {
        self.pending.position() as usize >= self.pending.get_ref().len()
    }
}

impl Read for Fed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.drained() {
            self.refill()?;
        }
        self.pending.read(buf)
    }
}

impl BufRead for Fed<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.drained() {
            self.refill()?;
        }
        self.pending.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.pending.consume(amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_that_says_nothing_is_waited_on_only_while_its_surface_is_open() {
        let (_quiet, heard) = mpsc::channel::<Vec<u8>>();
        let opened = Instant::now();
        let open = move || opened.elapsed() < Duration::from_millis(200);
        let mut fed = Fed {
            lines: heard,
            pending: std::io::Cursor::new(Vec::new()),
            open: &open,
        };
        let mut line = String::new();
        let waited = fed.read_line(&mut line);
        assert!(waited.is_err(), "{waited:?}");
        assert!(
            opened.elapsed() < Duration::from_secs(2),
            "it waited on the tenant"
        );
    }

    #[test]
    fn an_over_long_line_is_dropped_and_the_next_one_still_arrives() {
        let mut said = vec![b'x'; LINE_MOST as usize + 10];
        said.extend_from_slice(b"\nnext\n");
        let heard = listen(std::io::Cursor::new(said));
        assert_eq!(heard.recv().expect("a line"), b"next\n");
        assert!(heard.recv().is_err(), "and then the end");
    }
}
