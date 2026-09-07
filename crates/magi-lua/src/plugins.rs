//! Where configuration comes from, and in what order.
//!
//! neovim's model, unchanged: a runtimepath of roots, `plugin/` run at startup, `after/` last.
//! Twenty years of real plugins have been written against it and most people arriving already
//! know it. Deviating buys nothing and costs everyone the transfer.
//!
//! **This is balthasar's, generalised.** It was written there first, tested there, and described
//! nowhere as the family's answer — so the one program that could be extended by dropping a file
//! in a directory was the one nobody would think to look at for it. magi's loader said, in a
//! comment, that nothing was discovered by scanning and called that "the property a plugin
//! mechanism will need". Future tense, beside a working one in the next repository.
//!
//! **`init.lua` and `magi.load` are unchanged.** A named file is still the auditable case and
//! still the one a person should reach for; this adds the directories, it does not replace the
//! entry point. What is discovered runs *after* what was named, so a config that names everything
//! it wants behaves exactly as it did.
//!
//! **The sandbox covers all of it.** A discovered file runs in the same VM as a named one, and
//! the sandbox removes `os.execute`, `io.popen` and the rest before any of them run — so
//! dropping a file in a directory extends magi and cannot spawn a process.

use std::path::{Path, PathBuf};

/// Where a file came from, which is what decides what it may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// The owner's own configuration. May declare anything, and runs on sight.
    Owner,
    /// A package installed under `site/`. May declare anything the owner's files may — **once it
    /// has been acknowledged**.
    ///
    /// The distinction is not about what the file can express; it is about who wrote it. A file
    /// in your own `plugin/` directory is one you put there, and asking you to confirm your own
    /// configuration is a prompt nobody reads. A package is somebody else's code that arrived by
    /// being fetched, and it can change under you between one run and the next.
    Installed,
    /// A file that arrived with the project. May set a floor or add a section; may not name a
    /// command to run, an endpoint to send text to, or how somebody's transcripts are read.
    Project,
}

impl Trust {
    /// Whether a file at this level may declare.
    #[must_use]
    pub fn may_declare(self) -> bool {
        matches!(self, Self::Owner | Self::Installed)
    }

    /// Whether it has to be acknowledged before it runs.
    #[must_use]
    pub fn needs_acknowledging(self) -> bool {
        matches!(self, Self::Installed)
    }
}

/// The roots a configuration is read from.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    /// `$XDG_CONFIG_HOME/magi`, the owner's own.
    pub config: Option<PathBuf>,
    /// `$XDG_DATA_HOME/magi/site`, where installed packages live.
    pub site: Option<PathBuf>,
    /// The working directory, whose `.magi.lua` may choose but not declare.
    pub project: Option<PathBuf>,
}

impl Roots {
    /// The usual roots for a machine.
    ///
    /// No `given` root, unlike the siblings': magi is the one that does the coordinating, so
    /// there is nobody above it handing it a file.
    #[must_use]
    pub fn discovered(cwd: &Path) -> Self {
        Self {
            config: config_home().map(|home| home.join("magi")),
            site: data_home().map(|home| home.join("magi/site")),
            project: Some(cwd.to_owned()),
        }
    }
}

/// `$XDG_CONFIG_HOME`, or `~/.config`.
fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

/// `$XDG_DATA_HOME`, or `~/.local/share`.
fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

/// Every file to read, in the order to read it, with what each may do.
///
/// ```text
///   <config>/plugin/*.lua              alphabetical, each on its own
///   <site>/pack/*/start/*/plugin/*.lua installed packages
///   <config>/after/plugin/*.lua        the last word
///   ./.magi.lua                        may choose, may not declare
/// ```
///
/// `init.lua` is deliberately absent: the caller runs it first and drains what it named, and a
/// second copy of that decision here would be two places to change the entry point.
#[must_use]
pub fn runtimepath(roots: &Roots) -> Vec<(PathBuf, Trust)> {
    let mut out = Vec::new();

    if let Some(config) = &roots.config {
        out.extend(
            lua_files(&config.join("plugin"))
                .into_iter()
                .map(|path| (path, Trust::Owner)),
        );
    }

    if let Some(site) = &roots.site {
        for package in packages(&site.join("pack")) {
            out.extend(
                lua_files(&package.join("plugin"))
                    .into_iter()
                    .map(|path| (path, Trust::Installed)),
            );
        }
    }

    // `after/` runs last, which is what lets it win against keyed registrars: registering the
    // same identity twice replaces, so whoever registers last decides.
    if let Some(config) = &roots.config {
        out.extend(
            lua_files(&config.join("after/plugin"))
                .into_iter()
                .map(|path| (path, Trust::Owner)),
        );
    }

    if let Some(project) = &roots.project {
        let local = project.join(".magi.lua");
        if local.is_file() {
            out.push((local, Trust::Project));
        }
    }
    out
}

/// Whether a project directory is one the owner vouched for.
///
/// `magi.trusted = { "/home/you/work" }` in the owner's own configuration. A directory under a
/// vouched-for one counts, so vouching for a workspace does not mean listing every repository in
/// it. An empty string is not a root — it would `starts_with` every path there is.
#[must_use]
pub fn vouched_for(trusted: &[String], project: &Path) -> bool {
    trusted
        .iter()
        .any(|root| !root.is_empty() && project.starts_with(root))
}

/// Every `.lua` directly in a directory, alphabetically.
///
/// Alphabetical rather than by whatever the filesystem answers: a load order that changes between
/// machines is a configuration that behaves differently on each of them.
fn lua_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|end| end == "lua"))
        .collect();
    found.sort();
    found
}

/// Every installed package under `pack/*/start/*`.
fn packages(pack: &Path) -> Vec<PathBuf> {
    let Ok(groups) = std::fs::read_dir(pack) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for group in groups.flatten() {
        let Ok(entries) = std::fs::read_dir(group.path().join("start")) else {
            continue;
        };
        found.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir()),
        );
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-rtp", name)
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, "-- nothing\n").expect("write");
    }

    fn at(config: PathBuf) -> Roots {
        Roots {
            config: Some(config),
            site: None,
            project: None,
        }
    }

    #[test]
    fn dropping_a_file_in_plugin_is_enough_to_be_loaded() {
        // The whole point: no edit to `init.lua`, no `magi.load` naming it. This is what magi's
        // loader said it did not do.
        let root = scratch("dropped");
        let config = root.join("config");
        touch(&config.join("plugin/mine.lua"));

        let files = runtimepath(&at(config));
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].0.ends_with("plugin/mine.lua"));
        assert!(files[0].1.may_declare());
    }

    #[test]
    fn plugins_run_alphabetically_rather_than_in_directory_order() {
        // A load order that depends on what `read_dir` happens to answer is a configuration that
        // behaves differently on two machines with the same files.
        let root = scratch("order");
        let config = root.join("config");
        for name in ["zzz.lua", "aaa.lua", "mmm.lua"] {
            touch(&config.join("plugin").join(name));
        }
        let files = runtimepath(&at(config));
        let names: Vec<_> = files
            .iter()
            .filter_map(|(path, _)| path.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(names, ["aaa.lua", "mmm.lua", "zzz.lua"]);
    }

    #[test]
    fn after_gets_the_last_word() {
        // The registrars replace by name, so the file that runs last decides. `after/` is how a
        // person overrides something a package they installed declared.
        let root = scratch("after");
        let config = root.join("config");
        touch(&config.join("plugin/a.lua"));
        touch(&config.join("after/plugin/a.lua"));

        let files = runtimepath(&at(config));
        assert_eq!(files.len(), 2);
        assert!(files[0].0.ends_with("plugin/a.lua"));
        assert!(files[1].0.ends_with("after/plugin/a.lua"));
    }

    #[test]
    fn an_installed_package_is_read_between_the_two() {
        let root = scratch("pack");
        let config = root.join("config");
        let site = root.join("site");
        touch(&config.join("plugin/first.lua"));
        touch(&site.join("pack/vendor/start/thing/plugin/middle.lua"));
        touch(&config.join("after/plugin/last.lua"));

        let files = runtimepath(&Roots {
            config: Some(config),
            site: Some(site),
            project: None,
        });
        let names: Vec<_> = files
            .iter()
            .filter_map(|(path, _)| path.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(names, ["first.lua", "middle.lua", "last.lua"]);
    }

    #[test]
    fn a_project_file_arrives_last_and_may_not_declare() {
        // A `.magi.lua` comes with a checkout. Running it is fine; letting it name a command to
        // run is the thing cloning a repository must not be able to do.
        let root = scratch("project");
        let project = root.join("work");
        touch(&project.join(".magi.lua"));

        let files = runtimepath(&Roots {
            config: None,
            site: None,
            project: Some(project),
        });
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, Trust::Project);
        assert!(!files[0].1.may_declare());
    }

    #[test]
    fn nothing_installed_is_no_files_rather_than_an_error() {
        // The ordinary case for anybody who has not used this: every directory is absent.
        let root = scratch("empty");
        let files = runtimepath(&Roots {
            config: Some(root.join("nowhere")),
            site: Some(root.join("also-nowhere")),
            project: Some(root.join("nor-here")),
        });
        assert!(files.is_empty(), "{files:?}");
    }

    #[test]
    fn an_empty_trusted_entry_does_not_vouch_for_the_filesystem() {
        // `starts_with("")` is true of every path there is, so a stray empty string in
        // `magi.trusted` would silently trust every checkout on the machine.
        assert!(!vouched_for(&[String::new()], Path::new("/anywhere")));
        assert!(vouched_for(
            &["/home/you".to_owned()],
            Path::new("/home/you/work")
        ));
        assert!(!vouched_for(
            &["/home/you".to_owned()],
            Path::new("/home/other")
        ));
    }

    #[test]
    fn only_lua_files_are_picked_up() {
        // A README, a backup file or an editor's swap file in a plugin directory is not a plugin.
        let root = scratch("kinds");
        let config = root.join("config");
        touch(&config.join("plugin/real.lua"));
        touch(&config.join("plugin/README.md"));
        touch(&config.join("plugin/real.lua.bak"));

        let files = runtimepath(&at(config));
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].0.ends_with("real.lua"));
    }
}
