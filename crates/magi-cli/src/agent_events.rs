use magi_host::events::collection::branch::Branch;
use std::sync::{Mutex, OnceLock};

static PENDING: OnceLock<Mutex<Option<Branch>>> = OnceLock::new();

pub fn receive() -> anyhow::Result<()> {
    let branch = match Branch::receive_stdin() {
        Ok(branch) => Some(branch),
        Err(_) => {
            rustix::stdio::dup2_stdin(std::fs::File::open("/dev/null")?)?;
            magi_host::events::collection::runtime::unobserved_child();
            eprintln!("magi: child event bootstrap unavailable; continuing without collection");
            None
        }
    };
    PENDING
        .set(Mutex::new(branch))
        .map_err(|_| anyhow::anyhow!("child events already initialized"))
}

pub fn take() -> Option<Branch> {
    PENDING.get()?.lock().ok()?.take()
}
