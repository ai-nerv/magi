//! The same kernel jail casper builds for a tool command, here for `magi.shell` — the one path a
//! command becomes a process inside magi itself rather than in the tools program.
//!
//! Duplicated rather than shared: a library between magi and casper is the dependency `FAMILY.md`
//! exists to prevent. bubblewrap builds the world — a read-only machine, writable where the grants
//! say, credential stores masked, no network unless a reach grant asked. Beside it a seccomp filter
//! removes the syscalls no command needs, and where bubblewrap is absent Landlock adds the
//! filesystem, network and signal walls in-process — the containment that stands without a
//! namespace.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A `sh -c command` wrapped in the jail `write`/`reach` describe, rooted at `cwd`. Unwrapped when
/// bubblewrap is not installed — a loud downgrade, not a silent one.
#[must_use]
pub fn shell(command: &str, cwd: &Path, write: &[PathBuf], reach: bool) -> Command {
    let mut spawning = match which("bwrap") {
        Some(bwrap) => bwrapped(&bwrap, command, cwd, write, reach),
        None => {
            magi_model::noted!(
                "jail: bwrap is not installed; a shell command runs under seccomp and Landlock only"
            );
            let mut sh = Command::new("sh");
            sh.arg("-c").arg(command).current_dir(cwd);
            sh
        }
    };
    confine(&mut spawning);
    restrict(&mut spawning, cwd, write, reach);
    spawning
}

/// The `bwrap … -- sh -c command` invocation: the profile, a cleared environment kept down to the
/// few a command needs, then the command.
fn bwrapped(bwrap: &str, command: &str, cwd: &Path, write: &[PathBuf], reach: bool) -> Command {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut argv = profile(cwd, &home, write, reach);
    argv.push("--clearenv".to_owned());
    for keep in ["PATH", "HOME", "TERM", "LANG", "LC_ALL", "USER"] {
        if let Some(value) = std::env::var_os(keep).and_then(|v| v.into_string().ok()) {
            argv.extend(["--setenv".to_owned(), keep.to_owned(), value]);
        }
    }
    argv.extend([
        "--".to_owned(),
        "sh".to_owned(),
        "-c".to_owned(),
        command.to_owned(),
    ]);
    let mut jailed = Command::new(bwrap);
    jailed.args(&argv).current_dir(cwd);
    jailed
}

/// Deny the syscalls no shell command needs and a hostile one wants — reading another process's
/// memory, `io_uring` — as a `pre_exec` on `command`. Inherited across `exec`, so it holds for the
/// program bubblewrap runs and where there is none. The same list casper's jail forbids.
fn confine(command: &mut Command) {
    let filter = deny(FORBIDDEN);
    // SAFETY: the closure calls `apply_filter` and nothing else — one `prctl` pair between fork and
    // exec, built here and moved in so nothing is allocated in the child.
    #[allow(unsafe_code)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        command.pre_exec(move || {
            seccompiler::apply_filter(&filter)
                .map_err(|why| std::io::Error::other(format!("seccomp: {why}")))
        });
    }
}

/// The syscalls a jailed command may never make.
const FORBIDDEN: &[i64] = &[
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
];

/// A seccomp program allowing everything but `forbid`, each answered with `EPERM`.
fn deny(forbid: &[i64]) -> seccompiler::BpfProgram {
    use seccompiler::{SeccompAction, SeccompFilter, TargetArch};
    let rules = forbid.iter().map(|nr| (*nr, Vec::new())).collect();
    let arch = if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        TargetArch::x86_64
    };
    SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .and_then(std::convert::TryInto::try_into)
    .unwrap_or_default()
}

/// Restrict `command` with Landlock when bubblewrap will not build the world — the one path where
/// the filesystem, network and signal walls must stand without a mount namespace. A no-op when
/// bwrap is present (it contains the command instead) or on a kernel without Landlock.
fn restrict(command: &mut Command, cwd: &Path, write: &[PathBuf], reach: bool) {
    if which("bwrap").is_some() {
        return;
    }
    let Some(ruleset) = ruleset(cwd, write, reach) else {
        return;
    };
    // SAFETY: like `confine` — the ruleset is built and populated here; the closure only clones its
    // descriptor and calls `restrict_self`, which is `prctl` and one Landlock syscall, no allocation.
    #[allow(unsafe_code)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        command.pre_exec(move || {
            let status = ruleset
                .try_clone()
                .map_err(|why| std::io::Error::other(format!("landlock: {why}")))?
                .restrict_self()
                .map_err(|why| std::io::Error::other(format!("landlock: {why}")))?;
            if status.ruleset == landlock::RulesetStatus::NotEnforced {
                return Err(std::io::Error::other("landlock: not enforced"));
            }
            Ok(())
        });
    }
}

/// The system directories a command reads to run at all — never `$HOME`, so the credential stores
/// under it stay unreadable the way bubblewrap's mask makes them.
const SYSTEM_READ: &[&str] = &[
    "/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/opt", "/proc", "/sys", "/run",
];

/// A Landlock ruleset: read across the system directories, write at `cwd`, each `write` and the
/// scratch devices, TCP denied unless `reach`, and signals scoped to this domain. Best-effort, so an
/// older kernel keeps the walls it can and drops the rest rather than failing.
fn ruleset(cwd: &Path, write: &[PathBuf], reach: bool) -> Option<landlock::RulesetCreated> {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
        RulesetAttr, RulesetCreatedAttr, Scope,
    };
    let abi = ABI::V5;
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .ok()?
        .scope(Scope::Signal | Scope::AbstractUnixSocket)
        .ok()?;
    if !reach {
        ruleset = ruleset
            .handle_access(AccessNet::ConnectTcp | AccessNet::BindTcp)
            .ok()?;
    }
    let mut created = ruleset.create().ok()?;
    let (read, all) = (AccessFs::from_read(abi), AccessFs::from_all(abi));
    for dir in SYSTEM_READ {
        if let Ok(fd) = PathFd::new(dir) {
            created = created.add_rule(PathBeneath::new(fd, read)).ok()?;
        }
    }
    let writable = std::iter::once(cwd.to_path_buf())
        .chain(write.iter().cloned())
        .chain([PathBuf::from("/tmp"), PathBuf::from("/dev")]);
    for dir in writable {
        if let Ok(fd) = PathFd::new(&dir) {
            created = created.add_rule(PathBeneath::new(fd, all)).ok()?;
        }
    }
    Some(created)
}

/// The bubblewrap arguments: a read-only machine, writable at `cwd` and each `write` directory,
/// credentials masked, no network unless `reach`. In order, since a later mount wins.
#[must_use]
pub fn profile(cwd: &Path, home: &Path, write: &[PathBuf], reach: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let bind = |a: &mut Vec<String>, flag: &str, p: &Path| {
        a.push(flag.to_owned());
        a.push(p.display().to_string());
        a.push(p.display().to_string());
    };
    a.extend(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"].map(str::to_owned));
    a.push("--tmpfs".to_owned());
    a.push("/tmp".to_owned());
    bind(&mut a, "--bind", cwd);
    for dir in write {
        bind(&mut a, "--bind", dir);
    }
    for deny in [".ssh", ".aws", ".gnupg", ".docker", ".kube", ".config/gh"] {
        let path = home.join(deny);
        if path.exists() {
            a.push("--tmpfs".to_owned());
            a.push(path.display().to_string());
        }
    }
    let hooks = cwd.join(".git/hooks");
    if hooks.exists() {
        bind(&mut a, "--ro-bind", &hooks);
    }
    if !reach {
        a.push("--unshare-net".to_owned());
    }
    a.extend(["--unshare-pid", "--unshare-ipc", "--unshare-uts"].map(str::to_owned));
    a.extend(["--die-with-parent", "--new-session"].map(str::to_owned));
    a.push("--chdir".to_owned());
    a.push(cwd.display().to_string());
    a
}

/// The first `name` on `$PATH`.
fn which(name: &str) -> Option<String> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
            .map(|p| p.display().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn the_floor_reads_the_world_and_cuts_the_network() {
        let a = profile(Path::new("/w"), Path::new("/home/x"), &[], false).join(" ");
        assert!(a.contains("--ro-bind / /"), "{a}");
        assert!(a.contains("--bind /w /w"), "{a}");
        assert!(a.contains("--unshare-net"), "{a}");
    }

    #[test]
    fn a_reach_grant_keeps_the_network_and_a_write_grant_binds_the_dir() {
        let a = profile(
            Path::new("/w"),
            Path::new("/home/x"),
            &[PathBuf::from("/b")],
            true,
        )
        .join(" ");
        assert!(a.contains("--bind /b /b"), "{a}");
        assert!(!a.contains("--unshare-net"), "{a}");
    }

    #[test]
    fn the_seccomp_filter_denies_a_syscall() {
        // The mechanism, on a syscall a shell reaches easily: a filter denying `mkdir` blocks it,
        // applied the way `confine` applies it.
        use std::os::unix::process::CommandExt as _;
        let filter = deny(&[libc::SYS_mkdir, libc::SYS_mkdirat]);
        let dir = Scratch::new("magi-jail", "seccomp");
        let target = dir.join("nope");
        let mut command = Command::new("mkdir");
        command.arg(&target);
        // SAFETY: as in `confine` — one `apply_filter` between fork and exec.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                seccompiler::apply_filter(&filter).map_err(|_| std::io::Error::other("seccomp"))
            });
        }
        assert!(
            !command.status().expect("mkdir runs").success(),
            "the filter did not block mkdir"
        );
        assert!(!target.exists());
    }

    #[test]
    fn landlock_alone_denies_a_read_outside_the_set_and_keeps_the_cwd_writable() {
        // The degraded path: no bwrap, so Landlock is the only wall. A ruleset reading the system
        // and writing one directory, applied the way `restrict` applies it; a child then proves a
        // credential-shaped path outside the set is unreadable and the granted dir writable.
        use std::os::unix::process::CommandExt as _;
        let dir = Scratch::new("magi-jail", "landlock");
        let work = dir.join("work");
        let secret = dir.join("secret");
        std::fs::create_dir_all(&work).expect("mkdir");
        std::fs::write(&secret, "THE-SECRET-KEY").expect("write");
        let Some(ruleset) = ruleset(&work, &[], false) else {
            eprintln!("skipping: no Landlock here");
            return;
        };
        let script = format!(
            "cat {} 2>&1; echo --sep--; echo ok > {}/w 2>&1 && echo WROTE || echo NOWRITE",
            secret.display(),
            work.display()
        );
        let mut command = Command::new("sh");
        command.arg("-c").arg(&script);
        // SAFETY: as in `restrict` — clone the descriptor and `restrict_self`, syscalls only.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                ruleset
                    .try_clone()
                    .map_err(|why| std::io::Error::other(format!("landlock: {why}")))?
                    .restrict_self()
                    .map_err(|why| std::io::Error::other(format!("landlock: {why}")))?;
                Ok(())
            });
        }
        let out = command.output().expect("sh runs");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            !said.contains("THE-SECRET-KEY"),
            "the secret was readable under Landlock: {said}"
        );
        assert!(said.contains("WROTE"), "the cwd was not writable: {said}");
    }

    #[test]
    fn a_jailed_shell_reads_no_keys() {
        let Some(bwrap) = which("bwrap") else {
            eprintln!("skipping: no bwrap");
            return;
        };
        let dir = Scratch::new("magi-jail", "shell");
        let home = dir.join("home");
        let work = dir.join("work");
        std::fs::create_dir_all(home.join(".ssh")).expect("mkdir");
        std::fs::create_dir_all(&work).expect("mkdir");
        std::fs::write(home.join(".ssh/id"), "MAGI-SECRET").expect("write");
        // The profile's home is read from $HOME, so point it at the scratch for this one call.
        let mut argv = profile(&work, &home, &[], false);
        argv.extend([
            "--clearenv".to_owned(),
            "--setenv".to_owned(),
            "HOME".to_owned(),
            home.display().to_string(),
        ]);
        argv.extend([
            "--setenv".to_owned(),
            "PATH".to_owned(),
            "/usr/bin:/bin".to_owned(),
        ]);
        argv.extend([
            "--".to_owned(),
            "sh".to_owned(),
            "-c".to_owned(),
            "cat ~/.ssh/id 2>&1".to_owned(),
        ]);
        let out = Command::new(&bwrap)
            .args(&argv)
            .output()
            .expect("bwrap runs");
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("MAGI-SECRET"),
            "the key was readable in the jail: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
