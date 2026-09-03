//! A melchior that says what it was told to.
//!
//! The turn loop reaches a model by spawning melchior and reading a [`Said`] per line. A test
//! that wanted to drive a turn used to stand up a fake HTTP server and a recorded SSE stream;
//! now it writes a script that prints the answer it wants and points the broker at it.
//!
//! A script rather than a mock object, because what is being tested is the *spawn*: the argv,
//! the pipe, the framing and the fact that a closed stdin is what starts the turn. A mock in
//! process would agree with magi about all four and prove none of them.

use std::io::Write;
use std::path::{Path, PathBuf};

/// A stand-in melchior on disk.
///
/// Deleted when it drops, so a test that fails does not leave a program behind with a plausible
/// name. The directory is named for the test and the process, so two running at once cannot
/// take each other's.
pub struct Mind {
    dir: PathBuf,
}

impl Mind {
    /// A melchior that prints these lines, in order, and exits.
    ///
    /// Each is written as-is, so a test may say something malformed on purpose. A stream that
    /// ends without a terminal is the one case the broker has to name rather than hang on, and
    /// this is how that is arranged.
    ///
    /// # Panics
    /// When the script cannot be written, which is a broken test rather than a failing one.
    #[must_use]
    pub fn saying(name: &str, lines: &[&str]) -> Self {
        let dir = std::env::temp_dir().join(format!("magi-mind-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory for the fake melchior");

        let mut script = String::from("#!/bin/sh\n");
        // Read and discard the ask. melchior reads to end of file, and a fake that did not
        // would leave the broker's write blocking on a pipe nobody drains.
        script.push_str("cat > /dev/null\n");
        for line in lines {
            let quoted = line.replace('\'', "'\\''");
            script.push_str(&format!("printf '%s\\n' '{quoted}'\n"));
        }

        let path = dir.join("melchior");
        let mut file = std::fs::File::create(&path).expect("write the fake melchior");
        file.write_all(script.as_bytes()).expect("write");
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make it runnable");
        }
        Self { dir }
    }

    /// A melchior that answers with one message and stops.
    #[must_use]
    pub fn answering(name: &str, text: &str) -> Self {
        let said = serde_json::json!({ "said": "text", "text": text }).to_string();
        let stop = serde_json::json!({ "said": "stop", "reason": "end_turn" }).to_string();
        Self::saying(name, &[&said, &stop])
    }

    /// Where the program is, to hand to the broker.
    #[must_use]
    pub fn program(&self) -> &Path {
        // Leaked as a path rather than a name: nothing is put on `PATH`, so tests running
        // together cannot take each other's melchior.
        Box::leak(Box::new(self.dir.join("melchior")))
    }
}

impl Drop for Mind {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_melchior_is_runnable_and_prints_what_it_was_given() {
        let mind = Mind::answering("runnable", "hello");
        let out = std::process::Command::new(mind.program())
            .arg("ask")
            .arg("--json")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("it runs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("\"text\":\"hello\""), "{text}");
        assert!(text.contains("\"said\":\"stop\""), "{text}");
    }

    #[test]
    fn it_goes_when_it_is_dropped() {
        let path = {
            let mind = Mind::saying("dropped", &[]);
            mind.program().to_path_buf()
        };
        assert!(!path.exists(), "a fake melchior outlived its test");
    }
}
