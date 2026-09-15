//! What a spawned sibling said before it failed, and every step a session takes when somebody is
//! watching. A diagnosis goes to a file, and only when somebody asked for one: `$MAGI_DEBUG_LOG` for
//! this program alone, `$NERV_LOG` for the whole family in one file (`magi --logs` sets it).

/// This program's own log.
pub const VARIABLE: &str = "MAGI_DEBUG_LOG";

/// The family's shared log: every sibling started while it is set writes its steps into that file.
pub const FAMILY: &str = "NERV_LOG";

/// How each line names who wrote it.
const PROGRAM: &str = "magi";

/// Whether anybody asked for a log.
#[must_use]
pub fn enabled() -> bool {
    std::env::var_os(VARIABLE).is_some() || std::env::var_os(FAMILY).is_some()
}

/// Append one line to every log that was asked for, each once. Silent when a file cannot be opened.
pub fn note(args: std::fmt::Arguments<'_>) {
    let own = std::env::var_os(VARIABLE);
    let family = std::env::var_os(FAMILY).filter(|family| Some(family) != own.as_ref());
    for path in [own, family].into_iter().flatten() {
        note_to(std::path::Path::new(&path), args);
    }
}

/// The half that does not read the environment, so a test can exercise it: `set_var` is `unsafe`,
/// and `unsafe` is denied.
pub fn note_to(path: &std::path::Path, args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let line = stamped(&args.to_string(), std::time::SystemTime::now());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // One write per line, so lines from several programs appending to one file do not interleave.
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

/// One line as it lands: when, who, and what, kept to one line whatever it said.
fn stamped(said: &str, at: std::time::SystemTime) -> String {
    format!(
        "{} {PROGRAM}[{}] {}",
        timestamp(at),
        std::process::id(),
        said.replace('\n', " ⏎ ")
    )
}

/// A moment as UTC to the millisecond, `2026-09-15T14:03:07.123Z`, with no calendar crate.
#[must_use]
pub fn timestamp(at: std::time::SystemTime) -> String {
    let since = at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let seconds = since.as_secs();
    let days = i64::try_from(seconds / 86_400).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let clock = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        clock / 3600,
        clock % 3600 / 60,
        clock % 60,
        since.subsec_millis()
    )
}

/// Write one line to whichever logs were asked for, formatted like `println!`. A macro so the
/// arguments are not evaluated when nobody is listening.
#[macro_export]
macro_rules! noted {
    ($($arg:tt)*) => {
        if $crate::noted::enabled() {
            $crate::noted::note(format_args!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{FAMILY, VARIABLE, note_to, timestamp};
    use crate::scratch::Scratch;

    #[test]
    fn lines_are_appended_stamped_with_when_and_who() {
        let at = Scratch::file("magi-noted", "append", "log.txt");
        note_to(&at, format_args!("melchior: {} exited {}", "models", 1));
        note_to(&at, format_args!("and\nagain"));
        let held = std::fs::read_to_string(&at).expect("the log");
        let lines: Vec<&str> = held.lines().collect();
        assert_eq!(lines.len(), 2, "{held}");
        let who = format!(" magi[{}] ", std::process::id());
        assert!(lines[0].contains(&who), "{held}");
        assert!(lines[0].ends_with("melchior: models exited 1"), "{held}");
        assert!(
            lines[1].ends_with("and ⏎ again"),
            "one line, whatever it said"
        );
    }

    #[test]
    fn a_moment_is_written_as_utc_to_the_millisecond() {
        let epoch = std::time::UNIX_EPOCH;
        assert_eq!(timestamp(epoch), "1970-01-01T00:00:00.000Z");
        let leap = epoch + std::time::Duration::from_millis(951_782_400_250);
        assert_eq!(timestamp(leap), "2000-02-29T00:00:00.250Z");
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
            std::env::var_os(VARIABLE).is_none() && std::env::var_os(FAMILY).is_none(),
            "the suite sets no log"
        );
        let at = Scratch::file("magi-noted", "quiet", "log.txt");
        noted!("nobody asked");
        assert!(!at.exists(), "{}", at.display());
    }
}
