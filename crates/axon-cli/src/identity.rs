//! What one axon calls itself.
//!
//! `project/id` — the folder you are in, and which session in it. Two parts because sessions
//! talk to each other, and a name that says only "axon" answers neither of the questions a
//! message arriving from one would raise: *whose* work, and *which* of the several you have open.
//!
//! - **project** is the working directory's name, so it needs no configuration to be right. A
//!   `.axon.lua` may override it, and that is one of the few things a project file may say about
//!   itself: naming yourself carries no authority.
//! - **id** tells two sessions in one directory apart, and it is the whole of the local name —
//!   the socket in the project's runtime directory is called exactly this.
//!
//! There is no role in a name. Whether a session is a main or somebody's subagent is not
//! something it calls itself, it is something the tree knows: a session with a parent is a
//! subagent and a session without one is a main. Putting that in the name would have let a
//! session claim to be a main by being addressed as one.

/// What a session calls itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The working directory's name, or what a project file called it.
    pub project: String,
    /// Which session this is.
    pub id: String,
}

impl Identity {
    /// Work out who this session is.
    ///
    /// `named` is what a config called the project, if it said anything.
    #[must_use]
    pub fn here(named: Option<&str>) -> Self {
        let project = named
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(folder);
        Self {
            project,
            id: name(),
        }
    }

    /// The whole name, as it is shown and as another session would address it.
    #[must_use]
    pub fn full(&self) -> String {
        format!("{}/{}", self.project, self.id)
    }
}

/// The working directory's own name.
///
/// The last component, not the path: `/home/you/work/axon` is `axon`, because that is what a
/// person calls it. A directory with no name -- the root -- falls back to something rather than
/// to an empty half of a name.
fn folder() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "axon".to_owned())
}

/// The Greek alphabet, which is what an id is drawn from.
const GREEK: [&str; 24] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    "lambda", "mu", "nu", "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon", "phi", "chi",
    "psi", "omega",
];

/// A name for this session.
///
/// Drawn from the clock, which is enough: this distinguishes the handful of axons somebody has
/// open, not the rows of a database.
fn name() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize);
    from_seed(seed)
}

/// The name a seed picks, so the shape can be tested without waiting for a clock.
///
/// Always a pair — `delta-rho`, never `delta`. One word looks like a placeholder and reads as
/// though the second half went missing, and a single alphabet runs out at two dozen, which is
/// few enough that somebody with several projects open would meet a collision. The two never
/// match, because `rho-rho` reads as a bug in whatever printed it.
#[must_use]
fn from_seed(seed: usize) -> String {
    let first = seed % GREEK.len();
    // Offset from the first rather than chosen independently, so the two can never land on the
    // same word without throwing away the seeds that would have.
    let apart = 1 + (seed / GREEK.len()) % (GREEK.len() - 1);
    format!("{}-{}", GREEK[first], GREEK[(first + apart) % GREEK.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_two_parts() {
        let me = Identity::here(Some("thing"));
        assert_eq!(me.full(), format!("thing/{}", me.id));
        assert_eq!(me.full().split('/').count(), 2);
    }

    #[test]
    fn a_config_may_name_the_project_and_an_empty_name_is_not_a_name() {
        assert_eq!(Identity::here(Some("chosen")).project, "chosen");
        // Otherwise `axon.project = ""` produces `/delta-rho`, which reads as a bug.
        assert_eq!(Identity::here(Some("   ")).project, folder());
        assert_eq!(Identity::here(None).project, folder());
    }

    #[test]
    fn the_project_is_the_folder_rather_than_the_path() {
        // `/home/you/work/axon` is `axon`, because that is what a person calls it.
        assert!(!folder().contains('/'), "{}", folder());
        assert!(!folder().is_empty());
    }

    #[test]
    fn every_id_is_a_pair() {
        for seed in 0..2000 {
            let id = from_seed(seed);
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.len(), 2, "{id} is not a pair");
            assert!(GREEK.contains(&parts[0]), "{id}");
            assert!(GREEK.contains(&parts[1]), "{id}");
        }
        assert_eq!(from_seed(0), "alpha-beta");
    }

    #[test]
    fn the_two_halves_are_never_the_same_word() {
        // `rho-rho` reads as a bug in whatever printed it.
        for seed in 0..2000 {
            let id = from_seed(seed);
            let (first, second) = id.split_once('-').expect("a pair");
            assert_ne!(first, second, "{id}");
        }
    }

    #[test]
    fn there_are_enough_of_them_to_go_round() {
        // Two dozen was few enough that somebody with several projects open would collide.
        let mut seen: Vec<String> = (0..600).map(from_seed).collect();
        seen.sort_unstable();
        seen.dedup();
        assert!(seen.len() > 500, "only {} distinct names", seen.len());
    }

    #[test]
    fn a_name_never_contains_the_separator_it_is_joined_with() {
        // Otherwise `project/id` cannot be split back into two.
        for seed in 0..1000 {
            assert!(!from_seed(seed).contains('/'), "{}", from_seed(seed));
        }
    }
}
