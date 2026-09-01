//! What one axon calls itself.
//!
//! `project/role/id` — the folder you are in, what this session is for, and which session it is.
//! Three parts because sessions are about to talk to each other, and a name that says only
//! "axon" answers none of the questions a message arriving from one would raise: *whose* work,
//! doing *what*, and *which* of the several you have open.
//!
//! - **project** is the working directory's name, so it needs no configuration to be right. A
//!   `.axon.lua` may override it, and that is one of the few things a project file may say about
//!   itself: naming yourself carries no authority.
//! - **role** is `main` today. Multi-agent will fill it; the slot exists now so the shape of a
//!   name does not change when it does.
//! - **id** tells two sessions in one directory apart. Greek letters, because they are short,
//!   pronounceable over a desk, and ordered — `alpha` then `beta` reads as first and second.

/// The default role, until there is more than one.
const ROLE: &str = "main";

/// What a session calls itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The working directory's name, or what a project file called it.
    pub project: String,
    /// What this session is for.
    pub role: String,
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
            role: ROLE.to_owned(),
            id: name(),
        }
    }

    /// The whole name, as it is shown and as another session would address it.
    #[must_use]
    pub fn full(&self) -> String {
        format!("{}/{}/{}", self.project, self.role, self.id)
    }
}

/// The working directory's own name.
///
/// The last component, not the path: `/home/you/work/axon` is `axon`, because that is what a
/// person calls it. A directory with no name -- the root -- falls back to something rather than
/// to an empty half of a three-part name.
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
/// Drawn from the clock, which is enough: this distinguishes the two or three axons somebody has
/// open, not the rows of a database. Past twenty-four it pairs — `alpha-beta` — rather than
/// running out or falling back to a number nobody can say out loud.
fn name() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize);
    from_seed(seed)
}

/// The name a seed picks, so the pairing rule can be tested without waiting for a clock.
#[must_use]
fn from_seed(seed: usize) -> String {
    let first = GREEK[seed % GREEK.len()];
    // A second letter only when the seed asks for one, so the ordinary case is one short word.
    let paired = (seed / GREEK.len()) % (GREEK.len() + 1);
    match paired {
        0 => first.to_owned(),
        n => format!("{first}-{}", GREEK[n - 1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_three_parts() {
        let me = Identity::here(Some("thing"));
        assert_eq!(me.full(), format!("thing/main/{}", me.id));
        assert_eq!(me.full().split('/').count(), 3);
    }

    #[test]
    fn a_config_may_name_the_project_and_an_empty_name_is_not_a_name() {
        assert_eq!(Identity::here(Some("chosen")).project, "chosen");
        // Otherwise `axon.project = ""` produces `/main/alpha`, which reads as a bug.
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
    fn the_first_two_dozen_are_single_letters() {
        for seed in 0..24 {
            assert!(!from_seed(seed).contains('-'), "{}", from_seed(seed));
        }
        assert_eq!(from_seed(0), "alpha");
        assert_eq!(from_seed(1), "beta");
    }

    #[test]
    fn past_two_dozen_it_pairs_rather_than_repeating() {
        let paired = from_seed(24);
        assert!(paired.contains('-'), "{paired}");
        assert_eq!(paired, "alpha-alpha");
    }

    #[test]
    fn a_name_never_contains_the_separator_it_is_joined_with() {
        // Otherwise `project/role/id` cannot be split back into three.
        for seed in 0..1000 {
            assert!(!from_seed(seed).contains('/'), "{}", from_seed(seed));
        }
    }
}
