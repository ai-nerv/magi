//! Where configuration comes from, and in what order.
//!
//! neovim's model, unchanged: a runtimepath of roots, `plugin/` run at startup, `after/` last.
//! `init.lua` and `magi.load` are unchanged, and what is discovered runs after what was named. A
//! discovered file runs in the same sandboxed VM as a named one, so dropping a file in a directory
//! extends magi and cannot spawn a process.

use std::path::{Path, PathBuf};

/// Where a file came from, which is what decides what it may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// The owner's own configuration. May declare anything, and runs on sight.
    Owner,
    /// A package installed under `site/`. May declare anything the owner's files may, once it has
    /// been acknowledged: it is somebody else's code, and it can change between one run and the next.
    Installed,
    /// A file that arrived with the project. May set a floor or add a section; may not name a
    /// command to run, an endpoint to send text to, or how somebody's transcripts are read.
    Project,
}

impl Trust {
    #[must_use]
    pub fn may_declare(self) -> bool {
        matches!(self, Self::Owner | Self::Installed)
    }

    #[must_use]
    pub fn needs_acknowledging(self) -> bool {
        matches!(self, Self::Installed)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Roots {
    pub config: Option<PathBuf>,
    pub site: Option<PathBuf>,
    /// The working directory, whose `.magi.lua` may choose but not declare.
    pub project: Option<PathBuf>,
}

impl Roots {
    /// The usual roots for a machine. No `given` root, unlike the siblings': magi is the one that
    /// coordinates, so nobody above it hands it a file.
    #[must_use]
    pub fn discovered(cwd: &Path) -> Self {
        Self {
            config: config_home().map(|home| home.join("magi")),
            site: data_home().map(|home| home.join("magi/site")),
            project: Some(cwd.to_owned()),
        }
    }
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

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

    // `after/` runs last, which is what lets it win against keyed registrars: registering the same
    // identity twice replaces.
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

/// Whether a project directory is one the owner vouched for, by `magi.trusted` in the owner's own
/// configuration. A directory under a vouched-for one counts. An empty string is not a root — it
/// would `starts_with` every path there is.
#[must_use]
pub fn vouched_for(trusted: &[String], project: &Path) -> bool {
    trusted
        .iter()
        .any(|root| !root.is_empty() && project.starts_with(root))
}

/// Every `.lua` directly in a directory, alphabetically rather than by whatever the filesystem
/// answers: a load order that changes between machines is a configuration that does too.
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
        // No edit to `init.lua`, no `magi.load` naming it.
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
        // A load order that depends on `read_dir` behaves differently on two machines.
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
        // `after/` is how a person overrides something a package they installed declared.
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
        // Running a checkout's `.magi.lua` is fine; letting it name a command to run is not.
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
        // A stray empty string in `magi.trusted` would silently trust every checkout.
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
