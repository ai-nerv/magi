//! What a spawned sibling said before it failed.
//!
//! Every place in this program that starts another program nulls its stderr and turns a failure
//! into `None` or an empty list, so a diagnosis goes to `$MAGI_DEBUG_LOG` instead, and only when
//! somebody asked for one.

/// Append one line to `$MAGI_DEBUG_LOG`, if it is set. Silent when it is not, and silent when the
/// file cannot be opened.
pub fn note(args: std::fmt::Arguments<'_>) {
    let Some(path) = std::env::var_os(VARIABLE) else {
        return;
    };
    note_to(std::path::Path::new(&path), args);
}

pub const VARIABLE: &str = "MAGI_DEBUG_LOG";

/// The half that does not read the environment, so a test can exercise it. Mutating
/// `MAGI_DEBUG_LOG` from a test is not available: `set_var` is `unsafe`, and `unsafe` is denied.
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

/// Write one line to `$MAGI_DEBUG_LOG`, formatted like `println!`. A macro so the arguments are
/// not evaluated when the variable is unset.
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
        let at = Scratch::file("magi-noted", "append", "log.txt");
        note_to(&at, format_args!("melchior: {} exited {}", "models", 1));
        note_to(&at, format_args!("and again"));
        let held = std::fs::read_to_string(&at).expect("the log");
        assert_eq!(held, "melchior: models exited 1\nand again\n");
    }

    #[test]
    fn a_log_that_cannot_be_opened_is_not_an_error() {
        note_to(
            std::path::Path::new("/proc/nonexistent/nope"),
            format_args!("into the void"),
        );
    }

    #[test]
    fn nothing_is_written_when_nobody_asked() {
        assert!(
            std::env::var_os(VARIABLE).is_none(),
            "the suite sets no log"
        );
        let at = Scratch::file("magi-noted", "quiet", "log.txt");
        noted!("nobody asked");
        assert!(!at.exists(), "{}", at.display());
    }
}
