//! What magi knows about melchior, and what it does not.
//!
//! Split out under THE RULE; the two directions of the pipe are next door.

use super::*;

/// Where a session in `project` would bind its own UI socket.
///
/// Made the way a real one is rather than written out, because that is the whole of what is
/// being handed to melchior: a literal would prove the flag is accepted, not that what goes
/// through it is the path this process would bind.
fn a_screen(project: &str) -> std::path::PathBuf {
    crate::session::socket_for(project, &crate::session::key())
}

#[test]
fn a_missing_melchior_is_a_session_without_siblings() {
    // The balthasar rule: a sibling not being installed is the ordinary case, not a failure.
    // This is the one that decides whether somebody with no melchior can use magi at all.
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

/// What the spawn would say, without spawning anything.
///
/// The screen is handed in rather than made here, because [`crate::session::key`] is a pid
/// and a clock: two calls in one process give two paths, and a test that made its own to
/// compare against would be comparing two different sessions.
fn argv(screen: &std::path::Path, role: Role<'_>) -> Vec<String> {
    serving("melchior", "magi", None, screen, role)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn every_session_tells_the_layer_where_its_screen_is() {
    // **The one a headless magi rests on.** melchior cannot work this path out — it is named
    // after a key this process keeps to itself — so `--ui` is the whole of how a peer learns
    // where to attach. Dropped for a session with no terminal of its own, which is the
    // plausible-looking change, a headless agent would be on every roster with nowhere to
    // look and nothing anywhere would say so.
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
    // Both or neither. melchior takes whichever source speaks first entirely, so a magi that
    // passed the name and dropped the sentence would leave the description to be picked up
    // from a config — a role nobody declared, described by somebody who never met it.
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

    // And nothing at all for a session that was not told, which is every session somebody
    // opened at a terminal: the flags are how the command line speaks, not a default.
    let quiet = argv(&screen, Role::default());
    assert!(!quiet.iter().any(|arg| arg == "--role"));
    assert!(!quiet.iter().any(|arg| arg == "--role-description"));
}

#[test]
fn a_session_nobody_minted_is_its_own_run() {
    // The ordinary case: somebody opened a terminal. A root's run is its own id, which is
    // the same thing melchior writes into `<project>/<id>.session` for it.
    assert_eq!(
        run_from(None, "magi/main/alpha-rho").as_deref(),
        Some("alpha-rho")
    );
}

#[test]
fn a_minted_session_belongs_to_the_run_that_started_it() {
    // And not to itself. A subagent that worked out its own run would start a second one
    // every time a coordinator spawned a coordinator, and its memory would land beside
    // nobody's.
    assert_eq!(
        run_from(Some("alpha-rho"), "magi/worker/iota-mu").as_deref(),
        Some("alpha-rho")
    );
}

#[test]
fn a_session_with_no_melchior_belongs_to_no_run() {
    // Nothing named it, so there is nothing to file it under — and balthasar's own fallback,
    // one directory per run, is what a harness that never heard of runs already gets.
    assert_eq!(run_from(None, ""), None);
    assert_eq!(run_from(Some("   "), ""), None);
}

#[test]
fn a_prompt_naming_nobody_asks_melchior_nothing() {
    // Not merely empty — it must not *run* anything. A process per prompt, for a prompt that
    // named no instances, would be a spawn on every keystroke's worth of work.
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

    // A melchior too old to say which run it is in still names a session. The line that
    // starts one failing to parse is a magi that will not open at all, and the run has a
    // fallback where the name has none.
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
    // The other half of the wire, byte for byte as melchior writes it. Nothing fails when a
    // field name drifts: the line parses, `agents` is empty, and the session simply has no
    // peers — which reads as nobody else being up.
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
    // An agent whose harness published no screen is a peer with a name, not a line magi
    // refuses to read.
    assert_eq!(around[1].id, "psi-eta");
    assert_eq!(around[1].ui, None);
}

#[test]
fn an_older_melchior_that_says_only_names_still_has_peers() {
    // The two programs are released apart, so every build of one meets a build of the other
    // that predates it. Read strictly, this is a `$` popup that offers nobody for the life
    // of the session, with nothing anywhere saying why — and a name with no role and no
    // screen is exactly what magi had before any of this existed.
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

    // And a melchior halfway between the two, which names its agents and says nothing about
    // where they draw.
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
    // Two repositories move apart. A `heard` this build has never seen should fail to parse
    // rather than land in the nearest arm — an unknown line read as a `message` would put
    // something in the transcript that nobody said.
    assert!(serde_json::from_str::<Heard>(r#"{"event":"whistling","tune":"…"}"#).is_err());
}

/// A second session in the same project, so there is somebody to be talked to.
///
/// A bare child rather than another [`Melchior`], on purpose: if the thing under test is broken,
/// the fixture must not be broken the same way.
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
    // The bug this is here for, and it is the whole feature: `start` read the first line
    // through a reader it then dropped, which closed the pipe. The name arrived, the session
    // looked healthy, and no message ever reached the transcript again -- one line heard,
    // then silence, with nothing anywhere saying so.
    let project = format!("magi-hears-{}", std::process::id());
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

    let Some((mut them, theirs)) = a_sibling(&project) else {
        eprintln!("melchior is not installed; skipping");
        return;
    };
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
    let _ = them.kill();
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
    // A pipe read once is not a pipe read: the failure that started this looked exactly like
    // a working session until the second thing arrived.
    let project = format!("magi-again-{}", std::process::id());
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
    let Some((mut them, theirs)) = a_sibling(&project) else {
        return;
    };

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
    let _ = them.kill();
    assert_eq!(seen, ["one", "two", "three"], "it stopped listening");
}

#[test]
fn what_the_session_is_doing_keeps_reaching_the_layer() {
    // The other direction, and the same failure mode: a channel that looks fine because the
    // first write succeeded. A sibling asking `status` is told whatever was last said, so
    // one that died after a message reads as a session frozen mid-turn forever.
    let project = format!("magi-doing-{}", std::process::id());
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

    let Some((mut them, theirs)) = a_sibling(&project) else {
        return;
    };
    let mut asking = Command::new("melchior");
    asking.args(["tool", "--verb", "status", "--who", id_of(&me)]);
    as_session(&mut asking, &theirs);
    let asked = asking.output().expect("melchior tool runs");
    let _ = them.kill();
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
