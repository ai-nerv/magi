//! Every model this session could talk to, as melchior described them, beside the parts a
//! [`crate::turn::Backend`] does not vary. Held by the session rather than re-read on each switch,
//! so `/model` picks among what this session actually started with.

use crate::turn::Backend;
use magi_proto::ask::Card;

#[derive(Debug, Clone)]
pub struct Catalog {
    /// Tool descriptions, as `(name, source)`.
    pub tools: Vec<(String, String)>,
    /// The family's client libraries, as `(name, source)`.
    pub clients: Vec<(String, String)>,
    pub cwd: std::path::PathBuf,
    pub cards: Vec<Card>,
    pub wants: magi_proto::ask::Wants,
    pub system: Option<String>,
    /// Whether the file tools refuse paths outside `cwd`.
    pub confine: bool,
    pub grants: Vec<magi_proto::permit::Grant>,
    /// The SHA-256 casper's program must hash to, if this configuration pinned one.
    pub casper: Option<String>,
    /// What this session tells casper to be, on every spawn. See [`crate::turn::Backend`].
    pub casper_configure: String,
    /// Which program owns the model, as `magi.melchior` named it. One name for the whole session.
    pub mind: String,
    pub environ: std::collections::BTreeMap<String, String>,
    /// What the configuration asked for, kept so a refusal can name it rather than the fallback.
    pub chosen: Option<String>,
}

impl Catalog {
    /// A catalog with nothing in it, for a session that has no model.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tools: Vec::new(),
            clients: Vec::new(),
            casper: None,
            casper_configure: String::new(),
            mind: crate::broker::MELCHIOR.to_owned(),
            environ: std::collections::BTreeMap::new(),
            cwd: std::env::temp_dir(),
            cards: Vec::new(),
            wants: magi_proto::ask::Wants::default(),
            system: None,
            chosen: None,
            confine: false,
            grants: Vec::new(),
        }
    }

    /// The backend for a model, by qualified or bare name. `None` when no such model exists, or
    /// when melchior says it is not ready — which [`Self::unusable`] explains.
    #[must_use]
    pub fn backend(&self, name: &str) -> Option<Backend> {
        let card = self.find(name)?;
        card.ready.then(|| Backend {
            tools: self.tools.clone(),
            clients: self.clients.clone(),
            environ: self.environ.clone(),
            cwd: self.cwd.clone(),
            model: card.id.clone(),
            mind: self.mind.clone(),
            casper: self.casper.clone(),
            casper_configure: self.casper_configure.clone(),
            wants: self.wants.clone(),
            context_window: card.context_window,
            system: self.system.clone(),
            confine: self.confine,
            grants: self.grants.clone(),
        })
    }

    /// Why a named model cannot be used. Apart from `backend` because "no such model" and "no key
    /// set for it" send a person to two different places.
    #[must_use]
    pub fn unusable(&self, name: &str) -> Option<String> {
        let card = self.find(name)?;
        (!card.ready).then(|| match &card.needs {
            Some(variable) => format!(
                "{name} is offered by {}, which is not configured: set {variable}",
                card.provider
            ),
            None => format!(
                "{name} is offered by {}, which melchior cannot reach",
                card.provider
            ),
        })
    }

    /// Every model in the catalog, ready or not, with what it would take. All of them, because the
    /// person asking has usually configured nothing.
    #[must_use]
    pub fn choices(&self) -> Vec<magi_proto::ModelChoice> {
        let mut out: Vec<magi_proto::ModelChoice> = self
            .cards
            .iter()
            .map(|card| magi_proto::ModelChoice {
                name: card.id.clone(),
                context_window: card.context_window.unwrap_or(0),
                requirement: match (&card.needs, card.ready) {
                    (_, true) => String::new(),
                    (Some(variable), _) => format!("set {variable}"),
                    (None, _) => format!("{} is not reachable", card.provider),
                },
                wants_vars: card.needs.clone().into_iter().collect(),
                reasoning: card.reasons,
            })
            .collect();
        // Ready ones first, then by name.
        out.sort_by(|a, b| {
            a.requirement
                .is_empty()
                .cmp(&b.requirement.is_empty())
                .reverse()
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    /// Every model that could be switched to right now, sorted. Ready ones only.
    #[must_use]
    pub fn usable(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .cards
            .iter()
            .filter(|card| card.ready)
            .map(|card| card.id.clone())
            .collect();
        out.sort();
        out
    }

    /// What the configuration asked for.
    #[must_use]
    pub fn chosen(&self) -> Option<String> {
        self.chosen.clone()
    }

    /// Resolve a name the way melchior would: the full id first, then a bare model name when
    /// exactly one card ends in it. Split at the first slash only, so
    /// `openrouter/anthropic/claude-sonnet-4.5` is one provider and one model.
    fn find(&self, name: &str) -> Option<&Card> {
        if let Some(card) = self.cards.iter().find(|card| card.id == name) {
            return Some(card);
        }
        let mut matched = self.cards.iter().filter(|card| card.name == name);
        let first = matched.next()?;
        // Ambiguous is not resolved by guessing: declaration order is nobody's choice.
        matched.next().is_none().then_some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, name: &str, provider: &str, ready: bool) -> Card {
        Card {
            id: id.to_owned(),
            provider: provider.to_owned(),
            name: name.to_owned(),
            api: "openai-completions".to_owned(),
            context_window: Some(1000),
            max_output: Some(100),
            reasons: false,
            ready,
            needs: if ready {
                None
            } else {
                Some("MAGI_TEST_NOT_SET".to_owned())
            },
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            cards: vec![
                card("local/a", "a", "local", true),
                card("local/b", "b", "local", true),
                card("paid/x", "x", "paid", false),
            ],
            ..Catalog::empty()
        }
    }

    #[test]
    fn a_ready_model_yields_a_backend() {
        let backend = catalog().backend("local/a").expect("a backend");
        assert_eq!(backend.model, "local/a");
        assert_eq!(backend.context_window, Some(1000));
    }

    #[test]
    fn a_bare_name_resolves_when_it_is_unambiguous() {
        assert_eq!(catalog().backend("b").expect("a backend").model, "local/b");
    }

    #[test]
    fn a_model_id_containing_a_slash_is_still_one_model() {
        let held = Catalog {
            cards: vec![card(
                "openrouter/anthropic/claude-sonnet-4.5",
                "anthropic/claude-sonnet-4.5",
                "openrouter",
                true,
            )],
            ..Catalog::empty()
        };
        let backend = held
            .backend("openrouter/anthropic/claude-sonnet-4.5")
            .expect("a backend");
        assert_eq!(backend.model, "openrouter/anthropic/claude-sonnet-4.5");
    }

    #[test]
    fn a_model_with_no_credential_is_refused_with_a_reason() {
        let why = catalog().unusable("paid/x").expect("a reason");
        assert!(why.contains("MAGI_TEST_NOT_SET"), "{why}");
        assert!(catalog().backend("paid/x").is_none());
    }

    #[test]
    fn a_model_that_does_not_exist_has_no_reason_to_give() {
        assert!(catalog().unusable("nobody/nothing").is_none());
        assert!(catalog().backend("nobody/nothing").is_none());
    }

    #[test]
    fn only_reachable_models_are_offered() {
        assert_eq!(catalog().usable(), vec!["local/a", "local/b"]);
    }

    #[test]
    fn every_model_is_listed_with_what_it_would_take() {
        let choices = catalog().choices();
        assert_eq!(choices.len(), 3, "all of them, ready or not");
        assert!(choices[0].requirement.is_empty(), "ready ones first");
        let paid = choices.iter().find(|c| c.name == "paid/x").expect("paid");
        assert!(paid.requirement.contains("MAGI_TEST_NOT_SET"));
    }

    #[test]
    fn a_bare_name_two_providers_share_is_refused_rather_than_guessed() {
        let held = Catalog {
            cards: vec![
                card("one/same", "same", "one", true),
                card("two/same", "same", "two", true),
            ],
            ..Catalog::empty()
        };
        assert!(
            held.backend("same").is_none(),
            "declaration order is not a choice anybody made"
        );
        assert!(held.backend("one/same").is_some());
    }

    #[test]
    fn a_catalog_remembers_what_was_asked_for() {
        let mut catalog = Catalog::empty();
        assert!(catalog.chosen().is_none());
        catalog.chosen = Some("anthropic/claude-sonnet-4-5".into());
        assert_eq!(
            catalog.chosen().as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }
}
