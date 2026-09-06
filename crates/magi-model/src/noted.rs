//! What a spawned sibling said before it failed.
//!
//! **Six places in this program start another program, and every one of them nulls its stderr
//! and turns a failure into `None` or `Vec::new()`.** That is right for the terminal — a line on
//! stderr lands in the middle of a frame — and it is why `magi models` can print nothing at all
//! and mean any of four different things: melchior is not installed, it exited non-zero, it
//! answered something that is not JSON, or it genuinely has no cards. The caller cannot tell,
//! and neither can the person reading the empty list.
//!
//! So the diagnosis goes to a file instead, and only when somebody asked for one. `MAGI_DEBUG_LOG`
//! is the variable the editor path already used for exactly this reason; this is the same
//! mechanism, moved somewhere every crate can reach it.
//!
//! Deliberately not `tracing`. Six call sites do not justify a dependency, a subscriber, an
//! initialisation order and a second way for the program to fail at start-up — in four
//! workspaces, none of which may depend on the others.

/// Append one line to `$MAGI_DEBUG_LOG`, if it is set.
///
/// Silent when it is not, and silent when the file cannot be opened: a diagnostic that could
/// fail the thing it is diagnosing is worse than no diagnostic.
pub fn note(args: std::fmt::Arguments<'_>) {
    let Some(path) = std::env::var_os(VARIABLE) else {
        return;
    };
    note_to(std::path::Path::new(&path), args);
}

/// Which variable turns this on.
pub const VARIABLE: &str = "MAGI_DEBUG_LOG";

/// The half that does not read the environment, so a test can exercise it.
///
/// Mutating `MAGI_DEBUG_LOG` from a test is not available: `set_var` is `unsafe` under this
/// edition and `unsafe` is denied across the workspace. Splitting the read from the write costs
/// one function and makes the writing testable, which is the half that can be wrong.
pub fn note_to(path: &std::path::Path, args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{args}");
    }
}

/// Write one line to `$MAGI_DEBUG_LOG`, formatted like `println!`.
///
/// A macro rather than a function so the arguments are not evaluated when the variable is unset,
/// which is every run but the one where somebody is looking.
#[macro_export]
macro_rules! noted {
    ($($arg:tt)*) => {
        if std::env::var_os($crate::noted::VARIABLE).is_some() {
            $crate::noted::note(format_args!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{VARIABLE, note_to};
    use crate::scratch::Scratch;

    #[test]
    fn lines_are_appended_rather_than_replacing_each_other() {
        // One run of a session writes several; a log that kept only the last would answer
        // "what happened" with "the last thing".
        let at = Scratch::file("magi-noted", "append", "log.txt");
        note_to(&at, format_args!("melchior: {} exited {}", "models", 1));
        note_to(&at, format_args!("and again"));
        let held = std::fs::read_to_string(&at).expect("the log");
        assert_eq!(held, "melchior: models exited 1\nand again\n");
    }

    #[test]
    fn a_log_that_cannot_be_opened_is_not_an_error() {
        // A diagnostic that fails the thing it is diagnosing is worse than no diagnostic.
        note_to(
            std::path::Path::new("/proc/nonexistent/nope"),
            format_args!("into the void"),
        );
    }

    #[test]
    fn nothing_is_written_when_nobody_asked() {
        // The macro is the guard: with the variable unset it does not reach `note` at all, and
        // no file appears anywhere. This is every run but the one where somebody is looking.
        assert!(
            std::env::var_os(VARIABLE).is_none(),
            "the suite sets no log"
        );
        let at = Scratch::file("magi-noted", "quiet", "log.txt");
        noted!("nobody asked");
        assert!(!at.exists(), "{}", at.display());
    }
}
