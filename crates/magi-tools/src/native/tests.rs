#![allow(clippy::expect_used)]
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

struct Retired {
    pid: Arc<AtomicU32>,
    ended: Arc<AtomicBool>,
}

impl Drop for Retired {
    fn drop(&mut self) {
        let pid = self.pid.load(Ordering::Acquire);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        self.ended.store(true, Ordering::Release);
    }
}

#[test]
fn producer_retirement_follows_native_reaping_and_failed_spawn() {
    for available in [true, false] {
        let pid = Arc::new(AtomicU32::new(0));
        let ended = Arc::new(AtomicBool::new(false));
        let packet = std::fs::File::open("/dev/null").expect("packet fixture");
        let (input, _peer) = UnixStream::pair().expect("input fixture");
        let handoff = Handoff::new(
            packet.into(),
            input,
            Retired {
                pid: pid.clone(),
                ended: ended.clone(),
            },
        );
        let mut command = Command::new(if available {
            "sh"
        } else {
            "/no-such-native-event-program"
        });
        command.args(["-c", "exec sleep 30"]);
        let child = spawn(command, Some(handoff));
        if available {
            let child = child.expect("native child");
            pid.store(child.id(), Ordering::Release);
            assert!(!ended.load(Ordering::Acquire));
            drop(child);
        } else {
            assert!(child.is_err());
        }
        assert!(ended.load(Ordering::Acquire));
    }
}
