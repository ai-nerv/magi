//! A melchior that says what it was told to.
//!
//! The turn loop reaches a model by spawning melchior and reading a [`magi_proto::ask::Said`]
//! per line. A test that wanted to drive a turn used to stand up a fake HTTP server and a
//! recorded SSE stream; now it writes a script that prints the answer it wants and points a
//! backend at it.
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
    path: PathBuf,
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
        Self::turns(name, &[lines])
    }

    /// A melchior that answers each successive ask differently.
    ///
    /// One turn is one ask, and several of magi's own behaviours only appear across two: an
    /// overflow is refused and then, once the conversation has been compacted, answered. The
    /// last entry stands for every ask after it, so a refusal followed by an answer needs two
    /// rather than one per round the turn happens to take.
    ///
    /// # Panics
    /// As [`Self::saying`].
    #[must_use]
    pub fn turns(name: &str, turns: &[&[&str]]) -> Self {
        let mut body = String::from("case \"$n\" in\n");
        for (nth, lines) in turns.iter().enumerate() {
            // The last arm is `*`, so it answers every ask from there on. A test that named one
            // turn would otherwise get a silent melchior on the second, which reads as a hang.
            let label = if nth + 1 == turns.len() {
                "*".to_owned()
            } else {
                nth.to_string()
            };
            body.push_str(&format!("{label})\n"));
            for line in *lines {
                body.push_str(&format!("printf '%s\\n' '{}'\n", quoted(line)));
            }
            body.push_str(";;\n");
        }
        body.push_str("esac\n");
        written(name, &body)
    }

    /// A melchior that answers with one message and stops.
    #[must_use]
    pub fn answering(name: &str, text: &str) -> Self {
        Self::saying(name, &[&text_line(text), &stop_line()])
    }

    /// A melchior that takes the ask and never answers it.
    ///
    /// For the turn a person interrupts, and for anything that has to be true *while* a turn is
    /// in flight. It sleeps rather than exiting, because a melchior that closed its pipe would
    /// end the turn by itself — which is the one thing these tests must not be able to mistake
    /// for the behaviour they are checking.
    #[must_use]
    pub fn silent(name: &str) -> Self {
        // Bounded, because nothing kills the child when the turn stops waiting for it. Long
        // enough that no test outlasts it, short enough to leave nothing behind.
        written(name, "sleep 30\n")
    }

    /// Every ask this melchior was given, in order, as it arrived.
    ///
    /// For the handful of claims that are about what magi *sends* rather than what it does with
    /// the answer — a system prompt that reached the struct and not the wire looks identical
    /// from the outside.
    ///
    /// # Panics
    /// Never: an unread file is an ask that has not happened yet, which is an empty string.
    #[must_use]
    pub fn heard(&self) -> String {
        std::fs::read_to_string(self.dir.join("asks")).unwrap_or_default()
    }

    /// Each ask on its own, in the order they arrived.
    ///
    /// For the claims that are about one ask rather than all of them: that a resumed session
    /// replays the earlier exchange is a statement about the *second* request, and a blob with
    /// both in it cannot tell you which half anything was in.
    #[must_use]
    pub fn asks(&self) -> Vec<String> {
        (0..self.asked())
            .filter_map(|nth| std::fs::read_to_string(self.dir.join(format!("ask.{nth}"))).ok())
            .collect()
    }

    /// How many times it was asked.
    #[must_use]
    pub fn asked(&self) -> usize {
        std::fs::read_to_string(self.dir.join("asks.count"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Where the program is, to name in a backend.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.path
    }

    /// The directory to put in front of `PATH` so `melchior` means this one.
    ///
    /// For the tests that drive the real binary: magi finds its siblings on `PATH` and nothing
    /// in a config can point it elsewhere, which is deliberate — a config that could name the
    /// program that owns the model could name anything. A child process gets its own
    /// environment, so this is a `PATH` for one `magi` rather than for the test runner.
    #[must_use]
    pub fn on_path(&self) -> &Path {
        &self.dir
    }
}

/// The one model a fake melchior offers.
///
/// One, because a catalog is not what these tests are about: a config says `magi.model` and
/// this is the name it has to say.
pub const MODEL: &str = "fake/one";

impl Drop for Mind {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One `Said::Text`, as melchior writes it.
#[must_use]
pub fn text_line(text: &str) -> String {
    serde_json::json!({ "said": "text", "text": text }).to_string()
}

/// One `Said::Stop`, which is what ends a turn.
#[must_use]
pub fn stop_line() -> String {
    stopped_line("end_turn")
}

/// The same, for a turn that ended some other way.
#[must_use]
pub fn stopped_line(reason: &str) -> String {
    serde_json::json!({ "said": "stop", "reason": reason }).to_string()
}

/// The two lines a tool call arrives as, and the stop that ends the round it was in.
///
/// Three rather than one, because that is how it comes off the wire: the name arrives before
/// the arguments do, and a round that asked for a tool ends with `tool_use` rather than with
/// the turn.
#[must_use]
pub fn call_lines(id: &str, name: &str, args: &str) -> Vec<String> {
    vec![
        serde_json::json!({ "said": "tool_call_start", "id": id, "name": name }).to_string(),
        serde_json::json!({ "said": "tool_call_args", "args": args }).to_string(),
        stopped_line("tool_use"),
    ]
}

/// One `Said::Failed`, which is how a refusal arrives.
#[must_use]
pub fn failed_line(message: &str, why: &str) -> String {
    serde_json::json!({ "said": "failed", "message": message, "why": why }).to_string()
}

/// One `Said::Retrying`, which is how a wait announces itself.
#[must_use]
pub fn retrying_line(attempt: u32, of: u32, seconds: f64) -> String {
    serde_json::json!({
        "said": "retrying",
        "attempt": attempt,
        "of": of,
        "seconds": seconds,
        "why": "overload",
    })
    .to_string()
}

/// A string as a single-quoted shell word.
fn quoted(line: &str) -> String {
    line.replace('\'', "'\\''")
}

/// One line of a family reply, carrying `rows`.
fn reply(rows: &[serde_json::Value]) -> String {
    serde_json::json!({ "ok": true, "n": rows.len(), "result": rows }).to_string()
}

/// The verbs a fake melchior answers besides `ask`.
///
/// A real melchior offers all of them, and magi asks all of them on the way up: what models
/// there are, what settings this sibling takes, and here is what I have decided. A fake that
/// only answered `ask` would be a session that never got as far as a turn.
fn surface() -> String {
    let cards = reply(&[serde_json::json!({
        "id": MODEL, "provider": "fake", "name": "one",
        "api": "anthropic-messages", "context_window": 200_000,
        "max_output": 4096, "reasons": true, "ready": true,
    })]);
    // Nothing declared, so a coordinator has nothing to send and nothing comes back refused.
    // What magi tells a sibling has its own tests; these are about turns.
    let nothing = reply(&[]);
    let applied = reply(&[serde_json::json!({ "set": [], "refused": [] })]);
    format!(
        "case \"$1\" in\n\
         models) printf '%s\\n' '{cards}'; exit 0 ;;\n\
         needs) printf '%s\\n' '{nothing}'; exit 0 ;;\n\
         configure) cat > /dev/null; printf '%s\\n' '{applied}'; exit 0 ;;\n\
         ask) ;;\n\
         *) exit 1 ;;\n\
         esac\n"
    )
}

/// Write `body` as the `ask` half of a runnable fake melchior.
fn written(name: &str, body: &str) -> Mind {
    let dir = std::env::temp_dir().join(format!("magi-mind-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory for the fake melchior");

    let mut script = String::from("#!/bin/sh\nhere=$(dirname \"$0\")\n");
    script.push_str(&surface());
    // Counted before it is read, so `$n` is this ask's number and every arm below can use it.
    script.push_str(
        "n=0\n\
         [ -f \"$here/asks.count\" ] && n=$(cat \"$here/asks.count\")\n\
         echo $((n + 1)) > \"$here/asks.count\"\n",
    );
    // Kept. melchior reads to end of file, and a fake that did not read would leave the
    // broker's write blocking on a pipe nobody drains. One file per ask as well as one for all
    // of them, because what a resumed session sends is a claim about the second ask alone.
    script.push_str("cat > \"$here/ask.$n\"\ncat \"$here/ask.$n\" >> \"$here/asks\"\n");
    script.push_str(body);

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
    runnable(&path);
    Mind { dir, path }
}

/// Wait until this script can actually be executed, and return once it has been.
///
/// `ETXTBSY`. Tests run in parallel, and between a `fork` in one thread and its `exec` the
/// child holds every descriptor the parent had — including the one another thread is still
/// writing this file through. The kernel then refuses to exec it, and the failure surfaces
/// wherever the fake is spawned: inside the broker, as a melchior that could not be started,
/// on whichever test happened to be unlucky.
///
/// Retrying at each spawn would mean teaching the code under test to retry, which is wrong: a
/// melchior that genuinely is not there must be reported at once. So it is settled here, before
/// the `Mind` is handed out. One successful exec proves no descriptor is left open on the file,
/// and nothing writes it again after this — so every spawn from here on succeeds.
///
/// The verb is one the script does not know, so the probe exits immediately and leaves no mark:
/// the surface answers before anything is counted or recorded.
fn runnable(path: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match std::process::Command::new(path)
            .arg("--probe")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(_) => return,
            Err(why) if why.raw_os_error() == Some(26) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{} stayed busy: something is holding it open for writing",
                    path.display()
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(why) => panic!("the fake melchior will not run: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a fake melchior says to one verb.
    fn said(mind: &Mind, args: &[&str]) -> String {
        let out = std::process::Command::new(mind.program())
            .args(args)
            .output()
            .expect("it runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn run(mind: &Mind, ask: &str) -> String {
        use std::process::{Command, Stdio};
        let mut child = Command::new(mind.program())
            .arg("ask")
            .arg("--json")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("it runs");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(ask.as_bytes())
            .expect("write");
        let out = child.wait_with_output().expect("output");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn a_fake_melchior_is_runnable_and_prints_what_it_was_given() {
        let mind = Mind::answering("runnable", "hello");
        let said = run(&mind, "");
        assert!(said.contains("\"text\":\"hello\""), "{said}");
        assert!(said.contains("\"said\":\"stop\""), "{said}");
    }

    #[test]
    fn each_turn_gets_its_own_answer_and_the_last_one_repeats() {
        let mind = Mind::turns(
            "successive",
            &[
                &[&failed_line("too long", "overflow")],
                &[&text_line("second"), &stop_line()],
            ],
        );
        assert!(run(&mind, "").contains("overflow"));
        assert!(run(&mind, "").contains("second"));
        assert!(run(&mind, "").contains("second"), "the last arm repeats");
        assert_eq!(mind.asked(), 3);
    }

    #[test]
    fn what_it_was_asked_is_kept_for_the_test_to_read() {
        let mind = Mind::answering("heard", "hi");
        run(&mind, "{\"model\":\"m\"}");
        assert!(mind.heard().contains("\"model\":\"m\""), "{}", mind.heard());
    }

    #[test]
    fn every_line_it_writes_is_one_melchior_could_have_written() {
        // The whole value of a fake is that it agrees with the real contract. A helper that
        // produced a line nothing can parse would make a passing test out of a broker that
        // skips what it cannot read.
        let mut lines = vec![
            text_line("a"),
            stop_line(),
            stopped_line("tool_use"),
            failed_line("no", "overflow"),
            retrying_line(1, 4, 0.5),
        ];
        lines.extend(call_lines("c1", "read", "{}"));
        for line in lines {
            serde_json::from_str::<magi_proto::ask::Said>(&line)
                .unwrap_or_else(|why| panic!("{line} is not a Said: {why}"));
        }
    }

    #[test]
    fn the_rest_of_the_surface_answers_in_the_familys_shape() {
        // magi asks all of these on the way up, and a reply that is not the family's shape is
        // read as an empty list -- which looks exactly like a melchior with no models.
        let mind = Mind::answering("surface", "hi");
        for verb in ["models", "needs"] {
            let out = said(&mind, &[verb, "--json"]);
            let reply: serde_json::Value = serde_json::from_str(&out).expect("a family reply");
            assert_eq!(reply["ok"], true, "{verb}: {reply}");
            assert!(reply["result"].is_array(), "{verb}: {reply}");
        }
        let cards = said(&mind, &["models", "--json"]);
        let reply: serde_json::Value = serde_json::from_str(&cards).expect("a reply");
        let card: magi_proto::ask::Card =
            serde_json::from_value(reply["result"][0].clone()).expect("a card");
        assert_eq!(card.id, MODEL);
        assert!(card.ready, "a model nobody can use is no model at all");
    }

    #[test]
    fn it_goes_when_it_is_dropped() {
        let path = {
            let mind = Mind::answering("dropped", "x");
            mind.program().to_path_buf()
        };
        assert!(!path.exists(), "a fake melchior outlived its test");
    }
}
