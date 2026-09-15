//! The one cap on what a tool may return. An uncapped result is permanent: it is replayed on every
//! subsequent request, it sits inside the `KEEP` tail that compaction preserves verbatim, and the
//! summariser is handed the same blob. Here rather than in the tools because every result of every
//! transport passes through [`crate::Registry::call`], the only place that is true of.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// The most lines a result may carry into the transcript.
const MAX_LINES: usize = 2_000;

/// The most bytes a result may carry, whichever limit is reached first: a minified bundle or a
/// single-line JSON blob is one line and megabytes, so a line count alone is not a bound.
const MAX_BYTES: usize = 50_000;

/// How much of the budget is spent on the beginning rather than the end. Both ends are kept and the
/// middle dropped: a file read wants its head, and a build that failed wants its tail.
const HEAD_SHARE: usize = 2;

/// Cap `content`, spilling the whole of it to a file when it does not fit. Returns the text the
/// model should see, with the spill path named in it.
#[must_use]
pub fn apply(tool: &str, content: String) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= MAX_LINES && content.len() <= MAX_BYTES {
        return content;
    }

    let head_lines = MAX_LINES / HEAD_SHARE;
    let tail_lines = MAX_LINES - head_lines;
    let head_bytes = MAX_BYTES / HEAD_SHARE;
    let tail_bytes = MAX_BYTES - head_bytes;

    let head = take(&lines, head_lines, head_bytes, End::Head);
    let tail = take(&lines, tail_lines, tail_bytes, End::Tail);
    let dropped = lines.len().saturating_sub(head.len() + tail.len());

    let spilled = spill(tool, &content);
    let note = match &spilled {
        Some(path) => format!(
            "… {dropped} of {} lines cut from the middle. The whole of it is at {}",
            lines.len(),
            path.display()
        ),
        None => format!("… {dropped} of {} lines cut from the middle", lines.len()),
    };

    let mut out = String::with_capacity(MAX_BYTES + note.len() + 2);
    for line in head {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&note);
    out.push('\n');
    for line in tail {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Which end of the output a slice is taken from.
enum End {
    Head,
    Tail,
}

/// As many whole lines from one end as fit in both budgets.
fn take<'a>(lines: &[&'a str], max_lines: usize, max_bytes: usize, end: End) -> Vec<&'a str> {
    let mut taken: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    let ordered: Box<dyn Iterator<Item = &&'a str>> = match end {
        End::Head => Box::new(lines.iter()),
        End::Tail => Box::new(lines.iter().rev()),
    };
    for line in ordered {
        if taken.len() >= max_lines || bytes + line.len() + 1 > max_bytes {
            break;
        }
        bytes += line.len() + 1;
        taken.push(line);
    }
    if matches!(end, End::Tail) {
        taken.reverse();
    }
    taken
}

/// How long a spilled result is kept. Without it the directory grows for the life of the machine,
/// because nothing else knows the files exist.
const SPILL_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Write the full output beside the others, and answer where it went. Outside the session directory
/// on purpose, so `read` cannot open it — `Ops` refuses paths outside the session root — and the
/// note names the path only. Best effort: a result that could not be spilled is still capped.
fn spill(tool: &str, content: &str) -> Option<PathBuf> {
    // Per user. The temporary directory is shared: a fixed name there belongs to whoever created it
    // first, and every later user either fails to write or writes into a directory somebody owns.
    let uid = rustix::process::getuid().as_raw();
    let dir = std::env::temp_dir().join(format!("magi-output-{uid}"));
    std::fs::create_dir_all(&dir).ok()?;
    expire(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let safe: String = tool
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let path = dir.join(format!("{safe}-{stamp}.txt"));
    let mut file = std::fs::File::create(&path).ok()?;
    file.write_all(content.as_bytes()).ok()?;
    Some(path)
}

/// Drop spills older than [`SPILL_TTL`], on the way to writing a new one, so it needs no timer and
/// no cleanup path of its own. Failures are ignored: another process may be doing the same thing.
fn expire(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > SPILL_TTL);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `apply` and delete whatever it spilled; the spill directory is shared.
    fn applied(tool: &str, content: String) -> String {
        let out = apply(tool, content);
        if let Some(path) = spilled_path(&out) {
            let path = std::path::PathBuf::from(path);
            let _ = std::fs::remove_file(&path);
            // The directory is left alone. Removing it raced: `spill` creates the directory and
            // then the file, so a sibling test that emptied it in between broke `File::create`.
            // `gate-hermetic` exempts `magi-output-<uid>` by name.
        }
        out
    }

    /// The spill path named in a note, if there is one.
    fn spilled_path(out: &str) -> Option<String> {
        out.lines()
            .find_map(|l| l.split_once(" is at ").map(|(_, p)| p.to_owned()))
    }

    fn lines_of(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn output_that_fits_is_untouched() {
        let text = lines_of(10);
        assert_eq!(applied("bash", text.clone()), text);
    }

    #[test]
    fn too_many_lines_is_cut_in_the_middle() {
        let out = applied("bash", lines_of(10_000));
        assert!(out.contains("line 1\n"), "the head survives");
        assert!(out.contains("line 10000"), "and so does the tail");
        assert!(out.contains("cut from the middle"), "{out:.200}");
        assert!(out.lines().count() <= MAX_LINES + 1, "within the budget");
    }

    #[test]
    fn one_enormous_line_is_bounded_too() {
        // A minified bundle is one line and megabytes; a line count alone is not a bound.
        let out = applied("read", "x".repeat(MAX_BYTES * 4));
        assert!(out.len() < MAX_BYTES * 2, "{} bytes", out.len());
    }

    #[test]
    fn the_note_says_how_much_went_and_where() {
        let out = applied("bash", lines_of(10_000));
        let note = out
            .lines()
            .find(|l| l.contains("cut from the middle"))
            .expect("a note");
        assert!(note.contains("of 10000 lines"), "{note}");
        assert!(note.contains("/magi-output-"), "it names the spill: {note}");
    }

    #[test]
    fn the_spill_holds_the_whole_of_it() {
        let full = lines_of(10_000);
        let out = apply("bash", full.clone());
        let path = out
            .lines()
            .find_map(|l| l.split_once(" is at ").map(|(_, p)| p.to_owned()))
            .expect("a path");
        let spilled = std::fs::read_to_string(&path).expect("the spill file");
        assert_eq!(spilled, full);
        // The file, not the directory it is in — see `applied` for the race that caused.
        let _ = std::fs::remove_file(std::path::PathBuf::from(&path));
    }

    #[test]
    fn the_tail_is_kept_because_that_is_where_a_build_fails() {
        let mut text = lines_of(9_999);
        text.push_str("error: the thing that actually broke\n");
        let out = applied("bash", text);
        assert!(out.contains("the thing that actually broke"), "{out:.200}");
    }

    #[test]
    fn a_result_exactly_at_the_limit_is_left_alone() {
        let text = lines_of(MAX_LINES);
        assert_eq!(applied("bash", text.clone()), text);
    }
}

#[cfg(test)]
mod expiry_tests {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn a_stale_spill_is_dropped_on_the_way_past() {
        // Without this the directory grows for the life of the machine: nothing else knows the
        // files exist.
        let dir = Scratch::new("magi-expire", "one");
        let old = dir.join("old.txt");
        std::fs::write(&old, "x").expect("write");
        // Backdated past the ttl by touching, which is the only portable way to age a file.
        let stale = std::time::SystemTime::now() - SPILL_TTL - Duration::from_secs(60);
        let file = std::fs::File::options()
            .write(true)
            .open(&old)
            .expect("open");
        file.set_modified(stale).expect("backdate");

        expire(&dir);
        assert!(!old.exists(), "the stale one is gone");
    }

    #[test]
    fn a_fresh_spill_is_left_alone() {
        let dir = Scratch::new("magi-fresh", "one");
        let new = dir.join("new.txt");
        std::fs::write(&new, "x").expect("write");
        expire(&dir);
        assert!(
            new.exists(),
            "a spill named in this session's transcript survives"
        );
    }

    #[test]
    fn expiring_a_directory_that_is_not_there_is_not_an_error() {
        expire(&std::env::temp_dir().join("magi-does-not-exist-at-all"));
    }
}
