//! The directory lister the family's clients use to find each other.
//!
//! A sibling's client prefers `host.fs.ls(dir)` over shelling out to `io.popen`, because a
//! sandboxed host may refuse the latter. Offering it is what lets hexe's and oslo's clients
//! discover their own sockets while running inside axum.
//!
//! **`fs.dir` is deliberately not offered.** A client asks the host for "the directory my
//! sockets live in", and any host that answers gets believed — so axum answering would send
//! hexe's client looking for hexe sockets in axum's directory. Listing is generic and safe to
//! lend; naming your own runtime directory is not.

use luna::{Callback, CallbackReturn, Context, Table, Value};

/// Build the `fs` table.
pub fn table<'gc>(ctx: Context<'gc>) -> Table<'gc> {
    let fs = Table::new(&ctx);
    let ls = Callback::from_fn(&ctx, |ctx, _exec, mut stack| {
        let path: Value = stack.consume(ctx)?;
        let Value::String(path) = path else {
            stack.replace(ctx, Value::Nil);
            return Ok(CallbackReturn::Return);
        };
        let path = String::from_utf8_lossy(path.as_bytes()).into_owned();

        let out = Table::new(&ctx);
        // An unreadable directory is an empty listing, not a raise: a client probing several
        // candidate directories expects "nothing here", and most of them will not exist.
        if let Ok(entries) = std::fs::read_dir(&path) {
            let mut index = 1_i64;
            for entry in entries.flatten() {
                let record = Table::new(&ctx);
                let name = entry.file_name().to_string_lossy().into_owned();
                record
                    .set(ctx, "name", luna::String::from_slice(&ctx, name.as_bytes()))
                    .ok();

                // Modification time, because the client sorts by it to prefer the newest session.
                // Absent rather than zero when the filesystem will not say: zero would sort as
                // the oldest, which is a different claim from "unknown".
                if let Some(mtime) = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                {
                    record.set(ctx, "mtime", mtime.as_secs() as i64).ok();
                }
                out.set(ctx, index, record).ok();
                index += 1;
            }
        }
        stack.replace(ctx, out);
        Ok(CallbackReturn::Return)
    });
    fs.set(ctx, "ls", ls).ok();
    fs
}
