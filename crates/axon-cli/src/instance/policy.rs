//! Who may speak to whom.
//!
//! Two walls, and everything else is a setting.
//!
//! **The project wall.** A session can see and reach only what is inside its own project's
//! runtime directory. Not "should not" — cannot: the directory it lists and the directory it
//! dials are the one it belongs to, so an axon in `~/work/other` is not refused, it is not
//! there. Nothing in this file can turn that off, which is the point of putting it in the
//! filesystem rather than in a check.
//!
//! **The instance wall.** Inside a project, an *instance* is a main and the subagents it
//! started. A main is that instance's front door and is reachable by the other mains; the
//! subagents behind it are private, so `beta-nu`'s worker cannot be reached — or even
//! usefully named — by `alpha-rho`'s.
//!
//! ```text
//!   alpha-rho  <--->  beta-nu          two mains, two instances
//!      |                 |
//!      +- iota-mu        +- tau-chi     each main's own subagents
//!      +- zeta-pi        +- xi-phi
//! ```
//!
//! # What the setting moves
//!
//! The default is the tightest thing that still works: **mains talk to mains**, and a subagent
//! talks to whoever started it. That second one is not a peer relationship and no setting
//! governs it — a subagent that cannot report back to its parent cannot raise `attention` or
//! `trouble`, which is most of why it can speak at all.
//!
//! | `axon.agent_talk` | and also |
//! |---|---|
//! | `"mains"` *(default)* | — |
//! | `"instance"` | siblings: two subagents of the same parent |
//! | `"project"` | cousins, and a subagent reaching another instance's main |
//!
//! It only ever opens things. There is no level below `mains`, because a project where nothing
//! can talk is a project that did not need any of this.

use super::Reach;
use std::sync::OnceLock;

/// How far a session may reach, as the config set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Talk {
    /// Mains talk to mains. A subagent talks to its parent and no further.
    #[default]
    Mains,
    /// And siblings talk to each other.
    Instance,
    /// And anything in the project may reach anything else in it.
    Project,
}

impl Talk {
    /// Read what a config said, or `None` if it is not one of these.
    #[must_use]
    pub fn read(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "mains" => Self::Mains,
            "instance" => Self::Instance,
            "project" => Self::Project,
            _ => return None,
        })
    }

    /// What it is called, for saying so in a refusal.
    #[must_use]
    pub fn named(self) -> &'static str {
        match self {
            Self::Mains => "mains",
            Self::Instance => "instance",
            Self::Project => "project",
        }
    }
}

/// What the config chose, filled once at startup.
static CHOSEN: OnceLock<Talk> = OnceLock::new();

/// Take the config's answer, before anything asks.
///
/// The same `OnceLock` shape the UI settings use, and the same trap: the first *read* fills it
/// with the default, so this has to run before any call is answered. It does, because the socket
/// is bound after the config is loaded.
pub fn adopt(talk: Talk) {
    let _ = CHOSEN.set(talk);
}

/// How far a session may reach.
#[must_use]
pub fn talk() -> Talk {
    *CHOSEN.get_or_init(Talk::default)
}

/// Who a session is, as far as the tree is concerned.
///
/// Not what it calls itself — what can be found out about it from the project directory. The
/// parent is the whole of it: everything else in this file is derived from comparing two of
/// these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whom {
    /// Which project it belongs to.
    pub project: String,
    /// Its id, which is what its socket is called.
    pub id: String,
    /// Who started it, or `None` if it is a main.
    pub parent: Option<String>,
}

impl Whom {
    /// Whether this is a main — an instance's front door — rather than somebody's subagent.
    #[must_use]
    pub fn is_main(&self) -> bool {
        self.parent.is_none()
    }
}

/// How two sessions stand to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The same session. Reached by name rather than by knowing it was itself.
    Myself,
    /// It started me.
    Parent,
    /// I started it.
    Child,
    /// We were both started by the same session.
    Sibling,
    /// Another instance's main: a front door, in this project.
    Main,
    /// Another instance's subagent. Behind somebody else's front door.
    Cousin,
    /// A different project. Beyond the wall, and normally not even visible.
    Elsewhere,
}

impl Relation {
    /// What to call it, for saying so.
    #[must_use]
    pub fn named(self) -> &'static str {
        match self {
            Self::Myself => "this session",
            Self::Parent => "the session that started this one",
            Self::Child => "a subagent this session started",
            Self::Sibling => "a sibling subagent",
            Self::Main => "another instance's main",
            Self::Cousin => "another instance's subagent",
            Self::Elsewhere => "in another project",
        }
    }
}

/// How `them` stands to `me`.
#[must_use]
pub fn between(me: &Whom, them: &Whom) -> Relation {
    if me.project != them.project {
        return Relation::Elsewhere;
    }
    if me.id == them.id {
        return Relation::Myself;
    }
    if me.parent.as_deref() == Some(them.id.as_str()) {
        return Relation::Parent;
    }
    if them.parent.as_deref() == Some(me.id.as_str()) {
        return Relation::Child;
    }
    // Both have a parent and it is the same one. `None == None` would make every pair of mains
    // siblings, which is why the parent is unwrapped rather than compared as an option.
    if let (Some(mine), Some(theirs)) = (me.parent.as_deref(), them.parent.as_deref())
        && mine == theirs
    {
        return Relation::Sibling;
    }
    if them.is_main() {
        return Relation::Main;
    }
    Relation::Cousin
}

/// Whether `me` may do `reach` to something standing in `relation` to it.
///
/// The one place that answers "may I", so a verb added later cannot quietly forget to check.
#[must_use]
pub fn may(me: &Whom, relation: Relation, reach: Reach) -> bool {
    may_at(me, relation, reach, talk())
}

/// The same question with the setting handed in, so every level can be tested.
#[must_use]
pub fn may_at(me: &Whom, relation: Relation, reach: Reach, talk: Talk) -> bool {
    if reach == Reach::Stop {
        // The one act the far end cannot decline, so it is the one narrowed to the spawn link.
        // Even then it is not enough on its own: the caller has to hold the secret handed down
        // when the session was started, which is checked where the call is answered.
        return relation == Relation::Child;
    }
    match relation {
        // Never, at any setting. This is the project wall, and it is the reason there is no
        // level above `project`.
        Relation::Elsewhere => false,
        Relation::Myself | Relation::Parent | Relation::Child => true,
        Relation::Sibling => matches!(talk, Talk::Instance | Talk::Project),
        // A main is a front door to the other mains at every setting. A subagent knocking on
        // somebody else's front door is crossing the instance wall, so it waits for `project`.
        Relation::Main => me.is_main() || talk == Talk::Project,
        Relation::Cousin => talk == Talk::Project,
    }
}

/// Why it was refused, and what would have allowed it.
///
/// Says the setting by name, because "refused" with no way forward is how somebody concludes
/// the feature is broken rather than switched off.
#[must_use]
pub fn refusal(me: &Whom, relation: Relation, reach: Reach) -> String {
    refusal_at(me, relation, reach, talk())
}

/// The same, with the setting handed in.
#[must_use]
pub fn refusal_at(me: &Whom, relation: Relation, reach: Reach, talk: Talk) -> String {
    let verb = reach.named();
    if relation == Relation::Elsewhere {
        return format!(
            "that is {}, and axon does not reach across projects",
            relation.named()
        );
    }
    if reach == Reach::Stop {
        return format!(
            "only the session that started one may stop it, and that is {}",
            relation.named()
        );
    }
    let needed = match relation {
        Relation::Sibling => Talk::Instance,
        Relation::Cousin => Talk::Project,
        Relation::Main if !me.is_main() => Talk::Project,
        _ => Talk::Project,
    };
    format!(
        "this session may not {verb} {} while `axon.agent_talk` is \"{}\"; it would need \"{}\"",
        relation.named(),
        talk.named(),
        needed.named()
    )
}

/// The walls hold, and the setting only ever opens things.
#[cfg(test)]
mod tests {
    use super::*;

    use super::tests_support::{main_of, under};

    #[test]
    fn a_main_has_no_parent_and_a_subagent_does() {
        assert!(main_of("axon", "alpha-rho").is_main());
        assert!(!under("axon", "iota-mu", "alpha-rho").is_main());
    }

    #[test]
    fn two_mains_in_one_project_are_each_other_s_front_door() {
        let me = main_of("axon", "alpha-rho");
        let them = main_of("axon", "beta-nu");
        assert_eq!(between(&me, &them), Relation::Main);
    }

    #[test]
    fn the_spawn_link_reads_the_same_from_both_ends() {
        let parent = main_of("axon", "alpha-rho");
        let child = under("axon", "iota-mu", "alpha-rho");
        assert_eq!(between(&parent, &child), Relation::Child);
        assert_eq!(between(&child, &parent), Relation::Parent);
    }

    #[test]
    fn two_subagents_of_one_parent_are_siblings() {
        let one = under("axon", "iota-mu", "alpha-rho");
        let other = under("axon", "zeta-pi", "alpha-rho");
        assert_eq!(between(&one, &other), Relation::Sibling);
    }

    #[test]
    fn two_subagents_of_different_parents_are_cousins_not_siblings() {
        // The instance wall. Both are in one project and neither is behind the other's door.
        let mine = under("axon", "iota-mu", "alpha-rho");
        let theirs = under("axon", "tau-chi", "beta-nu");
        assert_eq!(between(&mine, &theirs), Relation::Cousin);
    }

    #[test]
    fn two_mains_are_not_siblings_for_both_having_no_parent() {
        // The bug an option comparison would have written: `None == None` makes every pair of
        // mains siblings, and then `"instance"` quietly becomes `"project"` for them.
        let me = main_of("axon", "alpha-rho");
        let them = main_of("axon", "beta-nu");
        assert_ne!(between(&me, &them), Relation::Sibling);
    }

    #[test]
    fn a_different_project_is_elsewhere_whoever_is_asking() {
        let me = main_of("axon", "alpha-rho");
        for them in [
            main_of("other", "beta-nu"),
            under("other", "tau-chi", "beta-nu"),
        ] {
            assert_eq!(between(&me, &them), Relation::Elsewhere);
        }
    }

    #[test]
    fn nothing_reaches_across_projects_at_any_setting() {
        // The project wall, and the reason it is enforced by the directory as well as here.
        let me = main_of("axon", "alpha-rho");
        for reach in [Reach::Ask, Reach::Tell, Reach::Stop] {
            assert!(!may(&me, Relation::Elsewhere, reach), "{reach:?} escaped");
        }
    }

    #[test]
    fn the_spawn_link_works_without_any_setting() {
        // The default is `mains`, and a subagent that cannot report back to its parent cannot
        // raise attention or trouble, which is most of why it can speak.
        let child = under("axon", "iota-mu", "alpha-rho");
        assert_eq!(talk(), Talk::Mains, "the default moved");
        assert!(may(&child, Relation::Parent, Reach::Tell));
        assert!(may(&child, Relation::Parent, Reach::Ask));
    }

    #[test]
    fn mains_reach_each_other_at_the_default() {
        let me = main_of("axon", "alpha-rho");
        assert!(may(&me, Relation::Main, Reach::Ask));
        assert!(may(&me, Relation::Main, Reach::Tell));
    }

    #[test]
    fn siblings_and_cousins_are_refused_at_the_default() {
        let child = under("axon", "iota-mu", "alpha-rho");
        assert!(!may(&child, Relation::Sibling, Reach::Tell));
        assert!(!may(&child, Relation::Cousin, Reach::Tell));
        assert!(
            !may(&child, Relation::Main, Reach::Tell),
            "a subagent knocking on another instance's door crosses the wall"
        );
    }

    #[test]
    fn only_the_parent_may_stop_a_session() {
        let me = main_of("axon", "alpha-rho");
        for relation in [
            Relation::Myself,
            Relation::Parent,
            Relation::Sibling,
            Relation::Main,
            Relation::Cousin,
            Relation::Elsewhere,
        ] {
            assert!(!may(&me, relation, Reach::Stop), "{relation:?} could stop");
        }
        assert!(may(&me, Relation::Child, Reach::Stop));
    }

    #[test]
    fn a_setting_that_is_not_one_is_not_read_as_a_looser_one() {
        // The failure that matters: a typo must not open the walls.
        assert_eq!(Talk::read("everything"), None);
        assert_eq!(Talk::read("Project"), None);
        assert_eq!(Talk::read("project"), Some(Talk::Project));
    }

    #[test]
    fn a_refusal_names_the_setting_that_would_have_allowed_it() {
        // Otherwise somebody concludes the feature is broken rather than switched off.
        let child = under("axon", "iota-mu", "alpha-rho");
        let said = refusal(&child, Relation::Sibling, Reach::Tell);
        assert!(said.contains("agent_talk"), "{said}");
        assert!(said.contains("instance"), "{said}");
    }

    #[test]
    fn a_refusal_across_projects_does_not_offer_a_setting_that_would_help() {
        // There is none, and suggesting one would be a lie.
        let me = main_of("axon", "alpha-rho");
        let said = refusal(&me, Relation::Elsewhere, Reach::Ask);
        assert!(!said.contains("agent_talk"), "{said}");
        assert!(said.contains("across projects"), "{said}");
    }
}

/// Each setting opens exactly what it says it does, and nothing beyond it.
#[cfg(test)]
mod levels {
    use super::tests_support::{main_of, under};
    use super::*;

    #[test]
    fn instance_opens_siblings_and_leaves_the_instance_wall_standing() {
        let child = under("axon", "iota-mu", "alpha-rho");
        assert!(may_at(
            &child,
            Relation::Sibling,
            Reach::Tell,
            Talk::Instance
        ));
        assert!(
            !may_at(&child, Relation::Cousin, Reach::Tell, Talk::Instance),
            "a cousin is behind another front door"
        );
        assert!(!may_at(&child, Relation::Main, Reach::Tell, Talk::Instance));
    }

    #[test]
    fn project_opens_everything_inside_the_project() {
        let child = under("axon", "iota-mu", "alpha-rho");
        for relation in [Relation::Sibling, Relation::Cousin, Relation::Main] {
            assert!(
                may_at(&child, relation, Reach::Tell, Talk::Project),
                "{relation:?} was still refused"
            );
        }
    }

    #[test]
    fn no_setting_opens_the_project_wall_or_widens_who_may_stop() {
        // The two things a config cannot buy.
        let me = main_of("axon", "alpha-rho");
        for talk in [Talk::Mains, Talk::Instance, Talk::Project] {
            assert!(!may_at(&me, Relation::Elsewhere, Reach::Ask, talk));
            assert!(!may_at(&me, Relation::Main, Reach::Stop, talk));
            assert!(!may_at(&me, Relation::Sibling, Reach::Stop, talk));
        }
    }

    #[test]
    fn each_step_only_ever_adds() {
        // A looser setting that refused something a tighter one allowed would be a trap.
        let who = [
            main_of("axon", "alpha-rho"),
            under("axon", "iota-mu", "alpha-rho"),
        ];
        let relations = [
            Relation::Myself,
            Relation::Parent,
            Relation::Child,
            Relation::Sibling,
            Relation::Main,
            Relation::Cousin,
        ];
        for me in &who {
            for relation in relations {
                for reach in [Reach::Ask, Reach::Tell] {
                    let tight = may_at(me, relation, reach, Talk::Mains);
                    let middle = may_at(me, relation, reach, Talk::Instance);
                    let loose = may_at(me, relation, reach, Talk::Project);
                    assert!(!tight || middle, "{relation:?} closed at instance");
                    assert!(!middle || loose, "{relation:?} closed at project");
                }
            }
        }
    }
}

/// Builders both test modules use.
#[cfg(test)]
mod tests_support {
    use super::Whom;

    pub fn main_of(project: &str, id: &str) -> Whom {
        Whom {
            project: project.to_owned(),
            id: id.to_owned(),
            parent: None,
        }
    }

    pub fn under(project: &str, id: &str, parent: &str) -> Whom {
        Whom {
            project: project.to_owned(),
            id: id.to_owned(),
            parent: Some(parent.to_owned()),
        }
    }
}
