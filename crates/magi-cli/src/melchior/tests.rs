//! What magi knows about melchior, and what it does not.

use super::*;

/// Where a session in `project` would bind its own UI socket, made the way a real one is: a literal
/// would prove the flag is accepted, not that what goes through it is the path this process binds.
fn a_screen(project: &str) -> std::path::PathBuf {
    crate::session::socket_for(project, &crate::session::key())
}

#[test]
fn a_missing_melchior_is_a_session_without_siblings() {
    // A sibling not being installed is the ordinary case, not a failure.
    assert!(
        Melchior::start(
            "melchior-that-is-not-installed",
            "magi",
            None,
            &a_screen("magi"),
            Role::default()
        )
        .is_none()
    );
}

/// What the spawn would say, without spawning anything. The screen is handed in because
/// [`crate::session::key`] is a pid and a clock: two calls in one process give two paths.
fn argv(screen: &std::path::Path, role: Role<'_>) -> Vec<String> {
    serving("melchior", "magi", None, screen, role)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn every_session_tells_the_layer_where_its_screen_is() {
    // melchior cannot work this path out — it is named after a key this process keeps to itself —
    // so `--ui` is the whole of how a peer learns where to attach.
    let screen = a_screen("magi");
    let said = argv(&screen, Role::default());
    let at = said
        .iter()
        .position(|arg| arg == "--ui")
        .expect("the layer is told where the screen is");
    assert_eq!(
        said.get(at + 1).map(String::as_str),
        Some(screen.to_string_lossy().as_ref()),
        "the layer would publish a path this process does not bind"
    );
}

#[test]
fn a_role_and_what_it_is_for_reach_the_layer_together() {
    // Both or neither: melchior takes whichever source speaks first entirely, so a name without
    // its sentence would leave the description to be picked up from a config.
    let screen = a_screen("magi");
    let said = argv(
        &screen,
        Role {
            name: Some("reviewer"),
            description: Some("reads diffs"),
        },
    );
    assert!(said.windows(2).any(|two| two == ["--role", "reviewer"]));
    assert!(
        said.windows(2)
            .any(|two| two == ["--role-description", "reads diffs"])
    );

    // And nothing at all for a session that was not told: the flags are the command line.
    let quiet = argv(&screen, Role::default());
    assert!(!quiet.iter().any(|arg| arg == "--role"));
    assert!(!quiet.iter().any(|arg| arg == "--role-description"));
}

#[test]
fn a_session_nobody_minted_is_its_own_run() {
    // The ordinary case: a root's run is its own id, which is what melchior writes for it too.
    assert_eq!(
        run_from(None, "magi/main/alpha-rho").as_deref(),
        Some("alpha-rho")
    );
}

#[test]
fn a_minted_session_belongs_to_the_run_that_started_it() {
    // And not to itself: a subagent working out its own run would start a second one each time.
    assert_eq!(
        run_from(Some("alpha-rho"), "magi/worker/iota-mu").as_deref(),
        Some("alpha-rho")
    );
}

#[test]
fn a_session_with_no_melchior_belongs_to_no_run() {
    // Nothing named it, so there is nothing to file it under.
    assert_eq!(run_from(None, ""), None);
    assert_eq!(run_from(Some("   "), ""), None);
}

#[test]
fn a_prompt_naming_nobody_asks_melchior_nothing() {
    // Not merely empty — it must not *run* anything: a process per prompt that named nobody.
    assert!(briefing("melchior-that-is-not-installed", "fix the parser", "magi").is_empty());
}

#[test]
fn a_briefing_from_a_missing_melchior_is_empty_rather_than_an_error() {
    assert!(briefing("melchior-that-is-not-installed", "ask $beta-nu", "magi").is_empty());
}

#[test]
fn what_melchior_says_is_read_as_what_it_means() {
    // The wire between two repositories, and the only place magi knows its shape.
    let listening: Heard = serde_json::from_str(
        r#"{"event":"listening","at":"/run/melchior/magi/psi-omicron","as":"magi/main/psi-omicron","run":"psi-omicron-1788913233"}"#,
    )
    .expect("reads");
    let Heard::Listening { at, named, run } = listening else {
        panic!("not a listening line");
    };
    assert_eq!(named, "magi/main/psi-omicron");
    assert_eq!(run, "psi-omicron-1788913233");
    assert!(at.ends_with("psi-omicron"));

    // A melchior too old to say which run it is in still names a session.
    let older: Heard = serde_json::from_str(
        r#"{"event":"listening","at":"/run/melchior/magi/psi-omicron","as":"magi/main/psi-omicron"}"#,
    )
    .expect("an older melchior still reads");
    assert!(matches!(older, Heard::Listening { run, .. } if run.is_empty()));

    let arrived: Heard = serde_json::from_str(
        r#"{"event":"message","who":"magi/main/beta-nu","sort":"attention","text":"look"}"#,
    )
    .expect("reads");
    let Heard::Message { who, sort, .. } = arrived else {
        panic!("not a message");
    };
    assert_eq!(who, "magi/main/beta-nu");
    assert_eq!(sort, "attention");
}

#[test]
fn a_roster_names_each_peer_with_its_role_and_its_screen() {
    // The other half of the wire, byte for byte as melchior writes it: a drifted field name leaves
    // `agents` empty rather than failing.
    let said: Heard = serde_json::from_str(
        r#"{"event":"around","agents":[{"id":"beta-nu","role":"reviewer","ui":"/run/user/1000/magi/magi/1f4a.host"},{"id":"psi-eta","role":"main","ui":null}]}"#,
    )
    .expect("reads");
    let Heard::Around { agents, names } = said else {
        panic!("not a roster");
    };
    let around = peers(agents, names);
    assert_eq!(around[0].id, "beta-nu");
    assert_eq!(around[0].role, "reviewer");
    assert_eq!(
        around[0].ui.as_deref(),
        Some(std::path::Path::new("/run/user/1000/magi/magi/1f4a.host"))
    );
    // An agent whose harness published no screen is a peer with a name.
    assert_eq!(around[1].id, "psi-eta");
    assert_eq!(around[1].ui, None);
}

#[test]
fn an_older_melchior_that_says_only_names_still_has_peers() {
    // The two programs are released apart, so every build of one meets one of the other that
    // predates it; read strictly this is a `$` popup that offers nobody.
    let said: Heard = serde_json::from_str(r#"{"event":"around","names":["beta-nu","psi-eta"]}"#)
        .expect("an older melchior still reads");
    let Heard::Around { agents, names } = said else {
        panic!("not a roster");
    };
    let around = peers(agents, names);
    assert_eq!(
        around
            .iter()
            .map(|them| them.id.as_str())
            .collect::<Vec<_>>(),
        ["beta-nu", "psi-eta"]
    );
    assert!(around.iter().all(|them| them.ui.is_none()));

    // And a melchior halfway between: names its agents, says nothing about where they draw.
    let said: Heard =
        serde_json::from_str(r#"{"event":"around","agents":[{"id":"beta-nu"}]}"#).expect("reads");
    let Heard::Around { agents, names } = said else {
        panic!("not a roster");
    };
    let around = peers(agents, names);
    assert_eq!(around[0].id, "beta-nu");
    assert_eq!(around[0].role, "");
    assert_eq!(around[0].ui, None);
}

#[test]
fn a_line_from_a_newer_melchior_is_not_read_as_something_it_is_not() {
    // A `heard` this build has never seen must fail to parse rather than land in the nearest arm.
    assert!(serde_json::from_str::<Heard>(r#"{"event":"whistling","tune":"…"}"#).is_err());
}

/// A project name nothing else will take, and the directory melchior files it under. Its runtime
/// directory cannot be pointed at a scratch from here — `set_var` is `unsafe` and this workspace
/// denies it — so the directory is taken away from a `Drop` afterwards instead.
struct Project(String);

impl Project {
    /// Named for the test and this process, which is what keeps two runs apart.
    fn named(what: &str) -> Self {
        Self(format!("magi-{what}-{}", std::process::id()))
    }
}

impl std::ops::Deref for Project {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        // Safe to remove wholesale: the name carries this pid, and both melchiors are gone by now.
        let _ = std::fs::remove_dir_all(runtime.join("melchior").join(&self.0));
    }
}

/// The second session, killed when the test ends rather than on its last line: a trailing
/// `let _ = them.kill()` does not run on the unwind, and a `kill` without a `wait` leaves a zombie.
struct Sibling(std::process::Child);

impl Drop for Sibling {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A second session in the same project. A bare child rather than another [`Melchior`], so a broken
/// thing under test cannot have a fixture broken the same way.
fn a_sibling(project: &str) -> Option<(std::process::Child, String)> {
    let mut child = Command::new("melchior")
        .args(["serve", "--project", project])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut said = String::new();
    BufReader::new(child.stdout.as_mut()?)
        .read_line(&mut said)
        .ok()?;
    let named = said
        .split("\"as\":\"")
        .nth(1)?
        .split('"')
        .next()?
        .to_owned();
    Some((child, named))
}

/// The three that tell a spawned process which session it is speaking as.
fn as_session(command: &mut Command, named: &str) {
    let mut parts = named.split('/');
    command
        .env("MAGI_MELCHIOR_PROJECT", parts.next().unwrap_or_default())
        .env("MAGI_MELCHIOR_ROLE", parts.next().unwrap_or_default())
        .env("MAGI_MELCHIOR_ID", parts.next().unwrap_or_default());
}

/// The last segment, which is what a sibling is addressed by inside one project.
fn id_of(named: &str) -> &str {
    named.rsplit('/').next().unwrap_or_default()
}

#[test]
fn a_session_keeps_hearing_after_the_line_that_named_it() {
    // `start` read the first line through a reader it then dropped, which closed the pipe: the name
    // arrived and no message reached the transcript again. Declared before the layer and the
    // sibling so it drops after both, since a directory removed mid-write comes straight back.
    let project = Project::named("hears");
    let Some((mut layer, _at)) = Melchior::start(
        "melchior",
        &project,
        None,
        &a_screen(&project),
        Role::default(),
    ) else {
        eprintln!("melchior is not installed; skipping");
        return;
    };
    let me = layer.named.clone();
    assert!(!me.is_empty(), "the layer must name the session");

    let mut heard = layer
        .hearing()
        .expect("the pipe is gone after start: nothing could ever arrive");

    let Some((them, theirs)) = a_sibling(&project) else {
        eprintln!("melchior is not installed; skipping");
        return;
    };
    let _them = Sibling(them);
    let mut sending = Command::new("melchior");
    sending.args([
        "tool",
        "--verb",
        "send",
        "--who",
        id_of(&me),
        "--message",
        "second line",
    ]);
    as_session(&mut sending, &theirs);
    let sent = sending.output().expect("melchior tool runs");
    assert!(
        sent.status.success(),
        "the send failed: {}",
        String::from_utf8_lossy(&sent.stderr)
    );

    // Past the roster, which melchior publishes whenever the set of sessions changes.
    let mut line = String::new();
    while heard.read_line(&mut line).is_ok_and(|read| read > 0) {
        if line.contains("\"message\"") {
            break;
        }
        line.clear();
    }
    assert!(
        line.contains("second line") && line.contains(&theirs),
        "the session heard: {line:?}"
    );
    // And it is the shape the driver turns into an entry, not merely text that mentions it.
    let said: Heard = serde_json::from_str(line.trim()).expect("a line magi can read");
    let Heard::Message { who, text, .. } = said else {
        panic!("not a message: {line}");
    };
    assert_eq!(who, theirs);
    assert_eq!(text, "second line");
}

#[test]
fn a_session_hears_every_message_rather_than_the_first() {
    // A pipe read once is not a pipe read: the failure looked exactly like a working session.
    let project = Project::named("again");
    let Some((mut layer, _at)) = Melchior::start(
        "melchior",
        &project,
        None,
        &a_screen(&project),
        Role::default(),
    ) else {
        return;
    };
    let me = layer.named.clone();
    let mut heard = layer.hearing().expect("the pipe");
    let Some((them, theirs)) = a_sibling(&project) else {
        return;
    };
    let _them = Sibling(them);

    for what in ["one", "two", "three"] {
        let mut sending = Command::new("melchior");
        sending.args([
            "tool",
            "--verb",
            "send",
            "--who",
            id_of(&me),
            "--message",
            what,
        ]);
        as_session(&mut sending, &theirs);
        assert!(sending.output().expect("runs").status.success());
    }

    let mut seen = Vec::new();
    let mut line = String::new();
    while seen.len() < 3 && heard.read_line(&mut line).is_ok_and(|read| read > 0) {
        if let Ok(Heard::Message { text, .. }) = serde_json::from_str::<Heard>(line.trim()) {
            seen.push(text);
        }
        line.clear();
    }
    assert_eq!(seen, ["one", "two", "three"], "it stopped listening");
}

#[test]
fn what_the_session_is_doing_keeps_reaching_the_layer() {
    // The other direction, and the same failure mode: a channel that looks fine after one write.
    let project = Project::named("doing");
    let Some((mut layer, _at)) = Melchior::start(
        "melchior",
        &project,
        None,
        &a_screen(&project),
        Role::default(),
    ) else {
        return;
    };
    let me = layer.named.clone();
    layer.doing(false, 0, 0);
    layer.doing(true, 41, 2);

    let Some((them, theirs)) = a_sibling(&project) else {
        return;
    };
    let _them = Sibling(them);
    let mut asking = Command::new("melchior");
    asking.args(["tool", "--verb", "status", "--who", id_of(&me)]);
    as_session(&mut asking, &theirs);
    let asked = asking.output().expect("melchior tool runs");
    let said = String::from_utf8_lossy(&asked.stdout).into_owned();
    assert!(
        asked.status.success(),
        "{}",
        String::from_utf8_lossy(&asked.stderr)
    );
    assert!(
        said.contains("41") || said.contains("working"),
        "the layer answered with the state at startup: {said}"
    );
}
