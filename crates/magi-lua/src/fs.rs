//! The directory lister the family's clients use to find each other.
//!
//! A sibling's client prefers `host.fs.ls(dir)` over shelling out to `io.popen`, which a sandboxed
//! host may refuse. `fs.write` runs through the same `Ops` the file tools do, and answers only
//! while the session's ops are lent, so a config cannot write while it is being read. `fs.dir` is
//! deliberately not offered: a client believes whatever host answers "the directory my sockets
//! live in", so magi answering would send hexe's client to magi's directory.

use luna::{Callback, CallbackReturn, Context, Table, Value};

pub fn table<'gc>(ctx: Context<'gc>, lent: crate::shell::Lent) -> Table<'gc> {
    let fs = Table::new(&ctx);
    let ls = Callback::from_fn(&ctx, |ctx, _exec, mut stack| {
        let path: Value = stack.consume(ctx)?;
        let Value::String(path) = path else {
            stack.replace(ctx, Value::Nil);
            return Ok(CallbackReturn::Return);
        };
        let path = String::from_utf8_lossy(path.as_bytes()).into_owned();

        let out = Table::new(&ctx);
        // An unreadable directory is an empty listing, not a raise: a client probes several.
        if let Ok(entries) = std::fs::read_dir(&path) {
            let mut index = 1_i64;
            for entry in entries.flatten() {
                let record = Table::new(&ctx);
                let name = entry.file_name().to_string_lossy().into_owned();
                record
                    .set(ctx, "name", luna::String::from_slice(&ctx, name.as_bytes()))
                    .ok();

                // Modification time, because the client sorts by it to prefer the newest session.
                // Absent rather than zero when the filesystem will not say.
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

    // Writing, through the same seam the file tools use. Two returns rather than a raise: taking
    // the session down because a watcher could not write would break the thing observed.
    let write = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
        let (path, contents): (Value, Value) = stack.consume(ctx)?;
        let (Value::String(path), Value::String(contents)) = (path, contents) else {
            stack.replace(
                ctx,
                (Value::Nil, "magi.fs.write(path, contents): two strings"),
            );
            return Ok(CallbackReturn::Return);
        };
        let path = String::from_utf8_lossy(path.as_bytes()).into_owned();
        let contents = String::from_utf8_lossy(contents.as_bytes()).into_owned();

        let held = lent.borrow();
        let Some(ops) = held.as_ref() else {
            // Config load time: nothing has been lent yet.
            stack.replace(ctx, (Value::Nil, "magi.fs.write is not available here"));
            return Ok(CallbackReturn::Return);
        };
        let action = magi_tools::permit::Action::Write { path: path.clone() };
        if let Err(why) = ops.allow("fs.write", &action) {
            stack.replace(ctx, (Value::Nil, why));
            return Ok(CallbackReturn::Return);
        }
        match ops.write(std::path::Path::new(&path), &contents) {
            Ok(()) => stack.replace(ctx, (true, Value::Nil)),
            Err(why) => {
                let why = luna::String::from_slice(&ctx, why.as_bytes());
                stack.replace(ctx, (Value::Nil, why));
            }
        }
        Ok(CallbackReturn::Return)
    });
    fs.set(ctx, "write", write).ok();
    fs
}

#[cfg(test)]
mod tests {
    use crate::Engine;

    #[test]
    fn writing_before_anything_is_lent_is_refused_rather_than_done() {
        // Config load time. Every config file in every checkout is read before anybody has been
        // asked anything.
        let mut engine = Engine::new();
        engine
            .run(
                r#"local ok, why = magi.fs.write("/tmp/magi-should-not-exist", "x")
                   magi.wrote = ok and "yes" or "no"
                   magi.why = why"#,
                "test",
            )
            .expect("runs");
        engine.harvest();
        let config = engine.config();
        assert_eq!(config.string("wrote"), Some("no"));
        assert!(
            config
                .string("why")
                .is_some_and(|why| why.contains("not available")),
            "{:?}",
            config.string("why")
        );
        assert!(
            !std::path::Path::new("/tmp/magi-should-not-exist").exists(),
            "and nothing was written"
        );
    }

    #[test]
    fn the_arguments_are_checked_before_anything_else() {
        let mut engine = Engine::new();
        engine
            .run(
                r#"local ok, why = magi.fs.write(1, 2) magi.why = why"#,
                "test",
            )
            .expect("runs");
        engine.harvest();
        assert!(
            engine
                .config()
                .string("why")
                .is_some_and(|why| why.contains("two strings")),
            "{:?}",
            engine.config().string("why")
        );
    }
}
