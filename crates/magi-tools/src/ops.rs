//! The seam every tool runs through: a tool never touches the filesystem or spawns a process
//! directly, it asks an [`Ops`], so execution can be redirected to an SSH host, a container or a
//! sandbox without touching a tool. It is also the safety boundary for Lua tools, whose reach is
//! decided here rather than by what the VM happens to expose.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Everything a tool is allowed to do to the outside world.
pub trait Ops: Send + Sync {
    /// Where relative paths resolve from.
    fn cwd(&self) -> PathBuf;

    /// The path a tool is about to act on, as the person should be asked about it: one string for
    /// the question and the deed, because a `Directory` grant is matched textually and two
    /// spellings of the same file would otherwise never meet. Lexical, like `resolve`, since a
    /// write to a new file has to be asked about before the file exists.
    fn resolved(&self, path: &Path) -> PathBuf {
        let joined = if path.is_absolute() {
            path.to_owned()
        } else {
            self.cwd().join(path)
        };
        real_path(&joined)
    }

    /// Read a file.
    ///
    /// # Errors
    /// When the path is outside the session, missing, or unreadable.
    fn read(&self, path: &Path) -> Result<String, String>;

    /// Write a file, creating parent directories.
    ///
    /// # Errors
    /// When the path is outside the session or the write fails.
    fn write(&self, path: &Path, contents: &str) -> Result<(), String>;

    /// Ask whether `action` may happen, blocking until it is answered. Called by a tool before it
    /// acts, not by the registry, because only the tool knows what it is about to do; `tool` is its
    /// own name rather than the verb, so a `grep` call does not say "read wants to read .". The
    /// default allows — [`Real`] is the one that gates.
    ///
    /// # Errors
    /// When it was refused, with a sentence the model reads as a result.
    fn allow(&self, tool: &str, action: &magi_proto::permit::Action) -> Result<(), String> {
        let _ = (tool, action);
        Ok(())
    }

    /// Take on grants a parent session already holds. `&self`, because the ledger is behind a lock:
    /// this arrives while the session is running. Nothing by default.
    fn take_on(&self, grants: Vec<magi_proto::permit::Grant>) {
        let _ = grants;
    }

    /// Every grant this session holds, for a child it starts to inherit. Nothing by default.
    fn held(&self) -> Vec<magi_proto::permit::Grant> {
        Vec::new()
    }

    /// Permission questions decided since the last time this was asked, read by the turn loop where
    /// the watchers are; see [`crate::watching::Pending`]. Nothing by default.
    fn noticed(&self) -> Vec<crate::watching::Noted> {
        Vec::new()
    }

    /// The jail profile a tools program is spawned with, as JSON; `None` when isolation is off.
    fn jail(&self) -> Option<String> {
        None
    }
}

/// Ops against the real machine, rooted at one directory. The root is where *relative* paths
/// resolve from — the session's directory, so `src/main.rs` means what it means in the shell you
/// started in. Not a wall by default: a rule only the careful tools obey moves work to `bash`.
/// Confinement is a configuration, `magi.confine = true`, and `bwrap` is what contains anything.
pub struct Real {
    root: PathBuf,
    confined: bool,
    /// Whether a tool command runs inside a kernel jail — `magi.isolation`. Independent of
    /// [`Self::confined`], the pre-flight path check: this contains a command that ignores it.
    isolate: bool,
    /// What has already been allowed, and who to ask when it has not.
    gate: Option<Gate>,
    /// Questions and answers, written down for whoever can be told; see [`crate::watching::Pending`].
    noticed: crate::watching::Pending,
}

/// The ledger and the person, together.
struct Gate {
    ledger: std::sync::Mutex<crate::permit::Ledger>,
    approver: std::sync::Arc<dyn crate::approve::Approver>,
}

impl Real {
    /// Ops rooted at `root`, reaching anywhere.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            confined: false,
            gate: None,
            isolate: false,
            noticed: crate::watching::Pending::new(),
        }
    }

    /// The same, asking `approver` about anything `ledger` does not already cover.
    #[must_use]
    pub fn gated(
        root: PathBuf,
        ledger: crate::permit::Ledger,
        approver: std::sync::Arc<dyn crate::approve::Approver>,
    ) -> Self {
        Self {
            root,
            confined: false,
            gate: Some(Gate {
                ledger: std::sync::Mutex::new(ledger),
                approver,
            }),
            isolate: false,
            noticed: crate::watching::Pending::new(),
        }
    }

    /// The grants this session has accumulated, for writing down.
    #[must_use]
    pub fn grants(&self) -> Vec<magi_proto::permit::Grant> {
        self.gate.as_ref().map_or_else(Vec::new, |gate| {
            gate.ledger
                .lock()
                .map(|l| l.persistent().to_vec())
                .unwrap_or_default()
        })
    }

    /// Ops that refuse anything outside `root`.
    #[must_use]
    pub fn confined(root: PathBuf) -> Self {
        Self {
            root,
            confined: true,
            gate: None,
            isolate: false,
            noticed: crate::watching::Pending::new(),
        }
    }

    /// Keep this one inside its root, or do not. A method rather than a fourth constructor:
    /// confinement and gating are independent, and the combinations grow by multiplication.
    #[must_use]
    pub fn confining(mut self, confined: bool) -> Self {
        self.confined = confined;
        self
    }

    /// Run tool commands inside a kernel jail, or do not.
    #[must_use]
    pub fn isolating(mut self, isolate: bool) -> Self {
        self.isolate = isolate;
        self
    }

    /// A directory to be the jail's shared `/tmp`, one per project so a project's commands and
    /// agents see each other's temp files. In the user's cache (`0700`) rather than a
    /// world-writable `/tmp/<name>` a stranger could pre-make, and on disk rather than under
    /// `$XDG_RUNTIME_DIR`, which is memory a single build can fill. Keyed by the session's
    /// directory. `None` when there is no cache to use, and casper falls back to a private tmpfs.
    fn shared_tmp(&self) -> Option<PathBuf> {
        use sha2::Digest;
        use std::os::unix::fs::PermissionsExt;
        let base = scratch_base()?.join("casper").join("tmp");
        let key = format!(
            "{:x}",
            sha2::Sha256::digest(self.root.to_string_lossy().as_bytes())
        );
        let dir = base.join(&key[..16]);
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).ok()?;
        Some(dir)
    }

    /// What the jail may write, from this session's write grants, and that it keeps the network:
    /// the jail contains the filesystem, and the network stays open.
    fn jail_reach(&self) -> (Vec<PathBuf>, bool) {
        use magi_proto::permit::Scope;
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        let mut write = Vec::new();
        for grant in self.grants() {
            if let ("write", Scope::Directory { path }) = (grant.verb.as_str(), &grant.scope) {
                let dir = PathBuf::from(path);
                if handed_to_jail(&dir, home.as_deref()) {
                    write.push(dir);
                } else {
                    magi_model::noted!("jail: write on {} is not handed over", dir.display());
                }
            }
        }
        (write, true)
    }

    /// Resolve a path against the root, refusing anything that escapes it when confined. Checked
    /// after normalising rather than on the text: `a/../../etc` has no leading `..` and a symlink
    /// has none at all.
    fn resolve(&self, path: &Path) -> Result<PathBuf, String> {
        let joined = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        let real = real_path(&joined);
        if !self.confined {
            return Ok(real);
        }
        let root = real_path(&self.root);
        if !real.starts_with(&root) {
            return Err(format!(
                "{} is outside this session's directory, and `magi.confine` is on",
                path.display()
            ));
        }
        Ok(real)
    }

    /// Open `real`, the path [`Self::resolve`] answered for `asked`: beneath the root when confined,
    /// through no link otherwise, and never through its leaf.
    fn open(
        &self,
        asked: &Path,
        real: &Path,
        flags: rustix::fs::OFlags,
        mode: u32,
    ) -> Result<std::fs::File, String> {
        let flags = flags | rustix::fs::OFlags::NOFOLLOW;
        let mode = rustix::fs::Mode::from_raw_mode(mode);
        let opened = if self.confined {
            let root = real_path(&self.root);
            let inside = real.strip_prefix(&root).unwrap_or(real);
            let inside = if inside.as_os_str().is_empty() {
                Path::new(".")
            } else {
                inside
            };
            opening::beneath(&root, inside, flags, mode)
        } else {
            opening::unlinked(real, flags, mode)
        };
        opened.map_err(|why| {
            if opening::escaped(&why) {
                format!(
                    "{} resolves outside this session's directory, and `magi.confine` is on",
                    asked.display()
                )
            } else {
                format!("{}: {why}", real.display())
            }
        })
    }

    /// The same action with any `Read`/`Write` path resolved against the root, so the gate sees
    /// what the deed will act on. `Run` and `Network` carry no path and pass through unchanged.
    fn resolved_action(&self, action: &magi_proto::permit::Action) -> magi_proto::permit::Action {
        use magi_proto::permit::Action;
        let resolved = |path: &str| self.resolved(Path::new(path)).display().to_string();
        match action {
            Action::Read { path } => Action::Read {
                path: resolved(path),
            },
            Action::Write { path } => Action::Write {
                path: resolved(path),
            },
            other => other.clone(),
        }
    }
}

/// Where the jails' shared temp directories go: `$XDG_CACHE_HOME`, else `~/.cache`. Disk, so what
/// a build leaves in `/tmp` costs no memory.
fn scratch_base() -> Option<PathBuf> {
    scratch_under(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

fn scratch_under(cache: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let set = |value: Option<OsString>| value.filter(|value| !value.is_empty());
    set(cache)
        .map(PathBuf::from)
        .or_else(|| set(home).map(|home| PathBuf::from(home).join(".cache")))
        .filter(|base| base.is_absolute())
}

/// Credentials and the family's own configuration, under `$HOME`. The jail is never handed write
/// on one of these, on anything inside one, or on any directory above one.
const PROTECTED: &[&str] = &[
    ".config/magi",
    ".config/casper",
    ".config/melchior",
    ".config/balthasar",
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".config/gcloud",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".password-store",
    ".local/share/keyrings",
];

/// Whether a write grant on `dir` may be handed to the jail: not `$HOME`, and not a directory
/// above, at or inside a `PROTECTED` one.
#[must_use]
pub fn handed_to_jail(dir: &Path, home: Option<&Path>) -> bool {
    let dir = real_path(dir);
    let Some(home) = home.map(real_path) else {
        return dir.parent().is_some();
    };
    !PROTECTED.iter().any(|held| {
        let held = home.join(held);
        held.starts_with(&dir) || dir.starts_with(&held)
    })
}

/// Where `path` lands on disk: its longest existing prefix resolved through every symlink, and
/// the rest appended. An absolute path in, an absolute path out.
#[must_use]
pub fn real_path(path: &Path) -> PathBuf {
    real_within(path, 40)
}

/// [`real_path`], following at most `hops` links that point at nothing yet.
fn real_within(path: &Path, hops: u32) -> PathBuf {
    let lexical = normalise(path);
    let mut existing = lexical.as_path();
    let mut rest = Vec::new();
    let joined = |at: PathBuf, rest: &[std::ffi::OsString]| {
        rest.iter().rev().fold(at, |at, part| at.join(part))
    };
    loop {
        if let Ok(real) = existing.canonicalize() {
            return joined(real, &rest);
        }
        // A link to something not there yet: a write through it creates what it names.
        if hops > 0
            && let Ok(target) = std::fs::read_link(existing)
        {
            let from = existing.parent().unwrap_or_else(|| Path::new("/"));
            return real_within(&joined(from.join(target), &rest), hops - 1);
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return lexical,
        }
    }
}

/// Whether a write to `path` changes something the next session, or the repository, runs:
/// a project's own configuration, or anything under `.git`.
#[must_use]
pub fn runs_later(path: &Path) -> bool {
    const RUN_ON_START: &[&str] = &[".magi.lua", ".balthasar.lua"];
    path.components().any(|part| part.as_os_str() == ".git")
        || path
            .file_name()
            .is_some_and(|name| RUN_ON_START.iter().any(|held| name == *held))
}

/// Resolve `.` and `..` without touching the filesystem. `canonicalize` would be stricter but
/// requires the path to exist, and a write to a new file has to be checked before it does.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

impl Ops for Real {
    fn cwd(&self) -> PathBuf {
        self.root.clone()
    }

    fn read(&self, asked: &Path) -> Result<String, String> {
        use std::io::Read;
        let path = self.resolve(asked)?;
        let mut text = String::new();
        self.open(asked, &path, rustix::fs::OFlags::RDONLY, 0)?
            .read_to_string(&mut text)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(text)
    }

    fn write(&self, path: &Path, contents: &str) -> Result<(), String> {
        use std::io::Write;
        let written = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        if std::fs::symlink_metadata(normalise(&written)).is_ok_and(|meta| meta.is_symlink()) {
            return Err(format!(
                "{} is a symbolic link; write to what it points at instead",
                path.display()
            ));
        }
        let asked = path;
        let path = self.resolve(asked)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        use rustix::fs::OFlags;
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC;
        self.open(asked, &path, flags, 0o666)?
            .write_all(contents.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Consult the ledger, and ask if it has nothing to say. The answer is recorded before it is
    /// acted on, so a person asked once about a directory is not asked again about the next file.
    fn allow(&self, tool: &str, action: &magi_proto::permit::Action) -> Result<(), String> {
        // The network is left unrestricted: it is essential, and a local dev server on the loopback
        // is reached the same as any host. The internal-address guard ([`crate::reaching`]) is the
        // ledger's to call when a session opts into confining its network, not the default.
        let Some(gate) = &self.gate else {
            return Ok(());
        };
        // Against the resolved path, as `read`/`write` act, or a relative one never meets a grant.
        let action = &self.resolved_action(action);
        if !gate.approver.overrides(action)
            && gate.ledger.lock().is_ok_and(|ledger| ledger.allows(action))
        {
            return Ok(());
        }
        let decision = gate.approver.ask(tool, action);
        if let Ok(mut ledger) = gate.ledger.lock() {
            ledger.remember(action, &decision);
        }
        self.noticed.note(crate::watching::Noted {
            verb: action.verb().to_string(),
            about: action.subject().to_string(),
            allowed: matches!(decision, magi_proto::permit::Decision::Allow { .. }),
        });
        match decision {
            magi_proto::permit::Decision::Allow { .. } => Ok(()),
            magi_proto::permit::Decision::Deny => Err(match gate.approver.why(action) {
                Some(why) => format!(
                    "not permitted: {} {}. {why}",
                    action.verb(),
                    action.subject()
                ),
                None => format!(
                    "not permitted: {} {}. No standing grant covers it, and asking got no leave: \
                     either it was declined, or nobody is attached to this session to be asked.",
                    action.verb(),
                    action.subject()
                ),
            }),
        }
    }

    fn noticed(&self) -> Vec<crate::watching::Noted> {
        self.noticed.drain()
    }

    /// The jail profile from the grants this session holds. `None` when isolation is off; otherwise
    /// the write directories, this project's spill directory, the network toggle, and a `/tmp`
    /// shared across this project's commands, conservative on an empty ledger, not open.
    fn jail(&self) -> Option<String> {
        if !self.isolate {
            return None;
        }
        let (mut write, reach) = self.jail_reach();
        write.extend(crate::bound::spill_dir(&self.root));
        let mut profile = serde_json::json!({ "write": write, "reach": reach });
        if let Some(tmp) = self.shared_tmp()
            && let Some(fields) = profile.as_object_mut()
        {
            fields.insert("tmp".to_owned(), serde_json::json!(tmp));
        }
        serde_json::to_string(&profile).ok()
    }

    fn take_on(&self, grants: Vec<magi_proto::permit::Grant>) {
        if let Some(gate) = &self.gate
            && let Ok(mut ledger) = gate.ledger.lock()
        {
            ledger.take_on(grants);
        }
    }

    fn held(&self) -> Vec<magi_proto::permit::Grant> {
        self.grants()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    fn rooted(name: &str) -> (Real, Scratch) {
        let dir = Scratch::new("magi-ops", name);
        (Real::new(dir.to_path_buf()), dir)
    }

    #[test]
    fn a_jails_shared_tmp_is_on_disk_in_the_users_cache() {
        let at = |cache: &str, home: &str| {
            let given = |value: &str| (!value.is_empty()).then(|| OsString::from(value));
            scratch_under(given(cache), given(home))
        };
        assert_eq!(at("/x/cache", "/home/u"), Some(PathBuf::from("/x/cache")));
        assert_eq!(at("", "/home/u"), Some(PathBuf::from("/home/u/.cache")));
        assert_eq!(at("relative", ""), None);
        assert_eq!(at("", ""), None);
    }

    #[test]
    fn isolation_off_is_no_jail_profile_at_all() {
        let (ops, _dir) = rooted("no-jail");
        assert_eq!(
            ops.jail(),
            None,
            "a session that did not ask for a jail gets none"
        );
    }

    #[test]
    fn a_write_grant_over_home_or_a_credential_is_not_handed_to_the_jail() {
        let home = Scratch::new("magi-ops", "home");
        std::fs::create_dir_all(home.join("work")).expect("mkdir");
        std::os::unix::fs::symlink(home.join(".ssh"), home.join("keys")).expect("symlink");
        let kept = |dir: &str| handed_to_jail(&home.join(dir), Some(&home));
        assert!(kept("work"));
        assert!(kept("work/build"));
        for refused in [
            "",
            ".config",
            ".config/magi",
            ".config/magi/plugin",
            ".ssh",
            ".local/share",
            "keys",
        ] {
            assert!(!kept(refused), "{refused:?} was handed over");
        }
        assert!(!handed_to_jail(Path::new("/"), Some(&home)));
        assert!(
            !handed_to_jail(Path::new("/"), None),
            "the root, with no home"
        );
    }

    #[test]
    fn the_network_stays_open_whatever_the_grants_say() {
        use magi_proto::permit::{Grant, Scope};
        for grants in [
            vec![],
            vec![Grant {
                verb: "reach".to_owned(),
                scope: Scope::Directory {
                    path: "example.test".to_owned(),
                },
            }],
        ] {
            let dir = Scratch::new("magi-ops", "network-floor");
            let ops = Real::gated(
                dir.to_path_buf(),
                crate::permit::Ledger::with(grants),
                std::sync::Arc::new(crate::approve::AllowAll),
            )
            .isolating(true);
            let profile: serde_json::Value =
                serde_json::from_str(&ops.jail().expect("profile")).expect("json");
            assert_eq!(profile["reach"], true);
            if let Some(tmp) = ops.shared_tmp() {
                let _ = std::fs::remove_dir_all(tmp);
            }
        }
    }

    #[test]
    fn the_jail_is_handed_this_projects_spill_directory_and_nothing_above_it() {
        let (ops, dir) = rooted("spills");
        let ops = ops.isolating(true);
        let profile: serde_json::Value =
            serde_json::from_str(&ops.jail().expect("a profile")).expect("json");
        let spills = crate::bound::spill_dir(&dir).expect("made");
        let write = profile["write"].as_array().expect("a write list");
        assert!(write.contains(&serde_json::json!(spills)), "{profile}");
        let above = serde_json::json!(spills.parent().expect("the user's"));
        assert!(!write.contains(&above), "{profile}");
        let _ = std::fs::remove_dir(&spills);
        if let Some(tmp) = ops.shared_tmp() {
            let _ = std::fs::remove_dir_all(tmp);
        }
    }

    #[test]
    fn the_jail_profile_is_built_from_the_grants() {
        use magi_proto::permit::{Grant, Scope};
        let ledger = crate::permit::Ledger::with(vec![
            Grant {
                verb: "write".to_owned(),
                scope: Scope::Directory {
                    path: "/w/build".to_owned(),
                },
            },
            Grant {
                verb: "reach".to_owned(),
                scope: Scope::Anything,
            },
        ]);
        let ops = Real::gated(
            std::path::PathBuf::from("/w"),
            ledger,
            std::sync::Arc::new(crate::approve::AllowAll),
        )
        .isolating(true);
        let json = ops.jail().expect("a profile when isolation is on");
        assert!(
            json.contains("/w/build"),
            "the write grant is in the profile: {json}"
        );
        assert!(
            json.contains("\"reach\":true"),
            "the reach grant opens the network: {json}"
        );
        assert!(json.contains("\"tmp\":"), "a shared /tmp is named: {json}");
        // The profile made a runtime directory; remove the one this test created.
        if let Some(tmp) = ops.shared_tmp() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }

    /// The same, with the wall on: `magi.confine` is where that rule lives now.
    fn walled(name: &str) -> (Real, Scratch) {
        let dir = Scratch::new("magi-wall", name);
        (Real::confined(dir.to_path_buf()), dir)
    }

    #[test]
    fn a_symlink_is_seen_through_before_gating_and_never_written_through() {
        let (ops, dir) = rooted("links");
        std::fs::create_dir_all(dir.join(".git/hooks")).expect("mkdir");
        std::os::unix::fs::symlink(dir.join(".git/hooks"), dir.join("h")).expect("symlink");
        std::os::unix::fs::symlink(dir.join(".git/config"), dir.join("c")).expect("symlink");
        let real = |path: &str| ops.resolved(Path::new(path));
        assert_eq!(real("h/post-checkout"), real(".git/hooks/post-checkout"));
        assert_eq!(real("c"), real(".git/config"), "a link to nothing yet");

        let refused = ops.write(Path::new("c"), "[core]").expect_err("refused");
        assert!(refused.contains("symbolic link"), "{refused}");
        assert!(
            !dir.join(".git/config").exists(),
            "the write went through the link"
        );

        let walled = Real::confined(dir.join("inner"));
        std::fs::create_dir_all(dir.join("inner")).expect("mkdir");
        std::os::unix::fs::symlink("/etc", dir.join("inner/x")).expect("symlink");
        assert!(
            walled.read(Path::new("x/hostname")).is_err(),
            "out through a link"
        );
    }

    #[test]
    fn a_write_then_a_read_round_trips() {
        let (ops, _dir) = rooted("roundtrip");
        ops.write(Path::new("a.txt"), "hello").expect("write");
        assert_eq!(ops.read(Path::new("a.txt")).expect("read"), "hello");
    }

    #[test]
    fn a_write_creates_parent_directories() {
        let (ops, dir) = rooted("parents");
        ops.write(Path::new("deep/nested/a.txt"), "x")
            .expect("write");
        assert!(dir.join("deep/nested/a.txt").exists());
    }

    #[test]
    fn a_confined_path_that_escapes_the_root_is_refused() {
        let (ops, _dir) = walled("escape");
        let error = ops
            .read(Path::new("../../etc/passwd"))
            .expect_err("must refuse");
        assert!(error.contains("outside"), "{error}");
    }

    #[test]
    fn an_escape_hidden_behind_a_descent_is_still_refused() {
        // `a/../../etc` has no leading `..` and still escapes, so the check happens after normalising.
        let (ops, _dir) = rooted("hidden");
        assert!(ops.read(Path::new("a/../../etc/passwd")).is_err());
    }

    #[test]
    fn a_confined_absolute_path_outside_the_root_is_refused() {
        let (ops, _dir) = walled("absolute");
        assert!(ops.read(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn reading_a_missing_file_names_it() {
        let (ops, _dir) = rooted("missing");
        let error = ops.read(Path::new("nope.txt")).expect_err("must fail");
        assert!(error.contains("nope.txt"), "{error}");
    }
}

#[cfg(test)]
mod reach_tests {
    use super::*;
    use magi_model::scratch::Scratch;

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-reach", name)
    }

    #[test]
    fn a_path_outside_the_session_is_reachable() {
        // The refusal was not safety: a model told "outside this session's directory" uses `bash`.
        let session = scratch("session");
        let elsewhere = scratch("elsewhere");
        let file = elsewhere.join("hello.py");
        std::fs::write(&file, "print('a')\n").expect("write");

        let ops = Real::new(session.to_path_buf());
        assert_eq!(ops.read(&file).expect("read"), "print('a')\n");
        ops.write(&file, "print('b')\n").expect("write");
        assert_eq!(
            std::fs::read_to_string(&file).expect("read back"),
            "print('b')\n"
        );
    }

    #[test]
    fn a_relative_path_still_means_the_session() {
        // The root's real job: `src/main.rs` means what it means in the shell you started in.
        let session = scratch("relative");
        std::fs::create_dir_all(session.join("src")).expect("mkdir");
        std::fs::write(session.join("src/main.rs"), "fn main() {}\n").expect("write");
        let ops = Real::new(session.to_path_buf());
        assert_eq!(
            ops.read(Path::new("src/main.rs")).expect("read"),
            "fn main() {}\n"
        );
    }

    #[test]
    fn confined_ops_still_refuse_and_say_why() {
        let session = scratch("wall");
        let ops = Real::confined(session.to_path_buf());
        let outside = std::env::temp_dir().join("magi-not-here.txt");
        let why = ops.read(&outside).expect_err("refused");
        assert!(why.contains("magi.confine"), "it names the setting: {why}");
    }

    #[test]
    fn confinement_still_catches_a_path_that_climbs_out() {
        let session = scratch("climb");
        let ops = Real::confined(session.to_path_buf());
        assert!(ops.read(Path::new("a/../../etc/passwd")).is_err());
    }
}

#[path = "ops/gating.rs"]
#[cfg(test)]
mod gating;

#[path = "ops/opening.rs"]
mod opening;
