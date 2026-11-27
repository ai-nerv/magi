//! Every model this session could talk to, as melchior described them, beside the parts a
//! [`Backend`] does not vary. Model cards change on explicit refresh; session configuration stays fixed.

use magi_proto::ask::Card;

pub mod setup;

/// Clamp a requested thinking level down to the closest level the model supports.
#[must_use]
pub fn clamp_thinking(
    requested: magi_model::ThinkingLevel,
    supported: &[magi_model::ThinkingLevel],
) -> magi_model::ThinkingLevel {
    use magi_model::ThinkingLevel;
    if supported.contains(&requested) {
        return requested;
    }
    let idx = ThinkingLevel::ALL
        .iter()
        .position(|l| *l == requested)
        .unwrap_or(0);
    ThinkingLevel::ALL
        .get(..idx)
        .unwrap_or_default()
        .iter()
        .rev()
        .find(|l| supported.contains(l))
        .copied()
        .unwrap_or(ThinkingLevel::Off)
}

/// The answer a request leaves room for when nothing configured says how long one may be.
pub(crate) const REPLY: u64 = 32_000;

/// What to set aside for the reply: what was asked for, else the default, and never more than the
/// model can say. Room reserved for an answer that cannot be given is room taken from the
/// conversation, and on a small window it was all of it.
pub(crate) fn reserved(wanted: Option<u64>, cap: Option<u64>) -> u64 {
    let asked = wanted.unwrap_or(REPLY);
    cap.filter(|cap| *cap > 0)
        .map_or(asked, |cap| asked.min(cap))
}

/// What the daemon needs to reach a model. Plain data, and sendable: the protocol it names is built
/// on the worker's own thread, because a Lua VM is neither `Send` nor `Sync`.
#[derive(Debug, Clone, Default)]
pub struct Backend {
    pub tools: Vec<(String, String)>,
    pub clients: Vec<(String, String)>,
    /// Which program fills the `tools` role, and what this session tells it.
    pub tooling: magi_tools::supplier::Tooling,
    pub cwd: std::path::PathBuf,
    /// Permissions a configuration granted in advance; they go into the ledger at startup.
    pub grants: Vec<magi_proto::permit::Grant>,
    /// Ephemeral grants supplied by the active profile; never persisted or inherited across switch.
    pub profile_grants: Vec<magi_proto::permit::Grant>,
    /// Tool visibility and execution filter for the active profile or agent.
    pub tool_filter: magi_tools::ToolFilter,
    /// Which models answer typed questions rather than writing, by id. Read from the catalog,
    /// since a helper may run on any of them and a backend carries no cards.
    pub deciders: Vec<String>,
    pub environ: std::collections::BTreeMap<String, String>,
    /// Whether the file tools refuse paths outside `cwd`. See [`magi_tools::ops::Real`].
    pub confine: bool,
    /// Whether a tool command runs inside a kernel jail — `magi.isolation`.
    pub isolate: bool,
    /// Which model to ask for, as melchior names it: `provider/model`. A name and nothing else.
    pub model: String,
    /// The program that owns the model, found on `PATH`. Named per backend, not compiled in.
    pub mind: String,
    pub wants: magi_proto::ask::Wants,
    /// How much this model will read, as melchior's card reported it. Carried rather than looked up.
    pub context_window: Option<u64>,
    /// The longest answer it can give, from the same card.
    pub max_output: Option<u64>,
    /// Resize limits for image inputs accepted by this model.
    pub image_resize: Option<magi_model::ImageResize>,
    pub session_identity: bool,
    pub transcript: Option<String>,
    /// What the model is told it is. Assembled once, when the daemon starts.
    pub system: Option<String>,
    /// The small models that run jobs on balthasar's behalf — `magi.helpers`.
    pub helpers: Helpers,
}

impl Backend {
    pub(crate) fn frame(&self) -> crate::frames::Request<'_> {
        crate::frames::Request {
            mind: &self.mind,
            model: &self.model,
            wants: &self.wants,
        }
    }
}

/// Which models help, and within what: a model per role, how long a job may take (zero is the
/// default), and the most one prompt may spend on them in millionths. Running one is `helping`'s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Helpers {
    pub roles: std::collections::BTreeMap<String, String>,
    pub timeout_ms: u64,
    pub per_prompt_micros: Option<u64>,
    /// Whether this session's conversation stays out of the project's notes: a child's prompts
    /// are its lead's instructions for one task, not what the person wants kept.
    pub no_notes: bool,
}

/// What one prompt's helper jobs have cost, in millionths: shared by every job it started, whether
/// blocking, beside the turn or after it, so the per-prompt cap holds for all of them.
pub type Spend = std::sync::Arc<std::sync::atomic::AtomicU64>;

#[derive(Debug, Clone)]
pub struct Catalog {
    /// Tool descriptions, as `(name, source)`.
    pub tools: Vec<(String, String)>,
    /// The family's client libraries, as `(name, source)`.
    pub clients: Vec<(String, String)>,
    pub cwd: std::path::PathBuf,
    /// Project workflow discovery root, relative to `cwd` or absolute.
    pub workflow_workspace: Option<std::path::PathBuf>,
    pub cards: Vec<Card>,
    pub wants: magi_proto::ask::Wants,
    pub system: Option<String>,
    /// Whether the file tools refuse paths outside `cwd`.
    pub confine: bool,
    /// Whether a tool command runs inside a kernel jail — `magi.isolation`.
    pub isolate: bool,
    pub grants: Vec<magi_proto::permit::Grant>,
    /// Who is asked about what no grant covers, and the rules no mode overrides.
    pub mode: magi_proto::judging::Mode,
    pub rules: magi_proto::judging::Rules,
    /// Which program fills the `tools` role, and what this session tells it. Beside [`Self::mind`]
    /// and [`Self::memory`] because it is the third of the same thing: a role, and who is doing it.
    pub tooling: magi_tools::supplier::Tooling,
    /// Which program owns the model, as `magi.melchior` named it. One name for the whole session.
    pub mind: String,
    /// Which program holds the history — the `memory` role, as `magi.memory` named it. Kept beside
    /// [`Self::mind`] because the client library it serves is filed under its own name.
    pub memory: String,
    pub environ: std::collections::BTreeMap<String, String>,
    /// What the configuration asked for, kept so a refusal can name it rather than the fallback.
    pub chosen: Option<String>,
    /// The small models that run jobs for balthasar and questions for surfaces — `magi.helpers`.
    pub helpers: Helpers,
    /// The key this session's transcript is recorded under, when not the session's own: a child
    /// of a run records `run@agent`, or the run's agents overwrite each other.
    pub transcript: Option<String>,
    /// Base system prompt prior to layering any profile prompt or preloaded skills.
    pub base_system: Option<String>,
    /// Base model prior to any profile override (restored when switching to a profile without `model`).
    pub base_model: Option<String>,
    /// Configured higher-level profiles (`magi.profiles` + `~/.config/magi/profiles/*.md`).
    pub profiles: Vec<crate::profiles::Profile>,
    /// Name of the currently active profile, if any.
    pub active_profile: Option<String>,
    /// Active skill allowlist (from active profile or child agent policy).
    pub skill_allow: Option<Vec<String>>,
    /// Active tool filter (from active profile or child agent policy).
    pub tool_filter: magi_tools::ToolFilter,
    /// Ephemeral grants from the active profile or child agent policy.
    pub profile_grants: Vec<magi_proto::permit::Grant>,
    /// Preloaded `(skill_name, body)` pairs attached to the active profile's system prompt.
    pub preloaded_skills: Vec<(String, String)>,
}

impl Backend {
    /// Whether `model` answers typed questions rather than writing, and so is sent a schema as a
    /// schema rather than in words.
    #[must_use]
    pub fn decides(&self, model: &str) -> bool {
        self.deciders.iter().any(|id| id == model)
    }
}

impl Catalog {
    #[must_use]
    pub fn thinking_levels(&self, name: &str) -> Option<Vec<magi_model::ThinkingLevel>> {
        let card = self.find(name)?;
        Some(magi_model::ThinkingLevel::available(
            card.reasons,
            card.capabilities.as_ref(),
        ))
    }

    /// A catalog with nothing in it, for a session that has no model.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tools: Vec::new(),
            clients: Vec::new(),
            tooling: magi_tools::supplier::Tooling::default(),
            mind: crate::broker::MELCHIOR.to_owned(),
            memory: crate::scribe::BALTHASAR.to_owned(),
            environ: std::collections::BTreeMap::new(),
            cwd: std::env::temp_dir(),
            workflow_workspace: None,
            cards: Vec::new(),
            wants: magi_proto::ask::Wants::default(),
            system: None,
            chosen: None,
            helpers: Helpers::default(),
            transcript: None,
            confine: false,
            isolate: true,
            grants: Vec::new(),
            mode: magi_proto::judging::Mode::default(),
            rules: magi_proto::judging::Rules::default(),
            base_system: None,
            base_model: None,
            profiles: Vec::new(),
            active_profile: None,
            skill_allow: None,
            tool_filter: magi_tools::ToolFilter::All,
            profile_grants: Vec::new(),
            preloaded_skills: Vec::new(),
        }
    }

    /// Look up the currently active profile, if one is set and present in [`Self::profiles`].
    #[must_use]
    pub fn active_profile_ref(&self) -> Option<&crate::profiles::Profile> {
        let name = self.active_profile.as_deref()?;
        self.profiles.iter().find(|p| p.name == name)
    }

    /// `(name, summary)` list of all configured profiles for the TUI `:profile` picker.
    #[must_use]
    pub fn profile_summaries(&self) -> Vec<(String, String)> {
        self.profiles
            .iter()
            .map(|p| (p.name.clone(), p.summary()))
            .collect()
    }

    /// Effective helper model map with active profile overrides layered on top of base helpers.
    /// An empty value disables a helper role (`models = { memory = false }`).
    #[must_use]
    pub fn effective_helpers(&self) -> Helpers {
        let mut helpers = self.helpers.clone();
        if let Some(profile) = self.active_profile_ref() {
            for (role, model) in &profile.helpers {
                if model.is_empty() {
                    helpers.roles.remove(role);
                } else {
                    helpers.roles.insert(role.clone(), model.clone());
                }
            }
        }
        helpers
    }

    /// Effective system prompt with active profile prompt and preloaded skills layered on top.
    #[must_use]
    pub fn effective_system(&self) -> Option<String> {
        let base = self.base_system.as_deref().or(self.system.as_deref());
        let profile = self.active_profile_ref();
        if profile.is_none() && self.preloaded_skills.is_empty() {
            return base.map(ToOwned::to_owned);
        }
        crate::profiles::compose_system(base, profile, &self.preloaded_skills)
    }

    /// Effective tooling configuration with active profile's allowed subagents applied.
    #[must_use]
    pub fn effective_tooling(&self) -> magi_tools::supplier::Tooling {
        let mut tooling = self.tooling.clone();
        if let Some(profile) = self.active_profile_ref()
            && let Some(allowed) = &profile.agents
        {
            tooling
                .kinds
                .retain(|kind| allowed.iter().any(|name| name == &kind.name));
            tooling.strict_kinds = true;
        }
        tooling
    }

    /// The backend for a model, by qualified or bare name. `None` when no such model exists, or
    /// when melchior says it is not ready — which [`Self::unusable`] explains.
    #[must_use]
    pub fn backend(&self, name: &str) -> Option<Backend> {
        let card = self.find(name)?;
        // What is reserved is what is asked for. Left unnamed, a provider generates to its own
        // default and the room held back for the reply describes nothing.
        let mut wants = self.wants.clone();
        if let Some(profile) = self.active_profile_ref()
            && let Some(level) = profile.thinking.as_deref()
            && let Ok(parsed) = serde_json::from_value::<magi_model::ThinkingLevel>(
                serde_json::Value::String(level.to_owned()),
            )
        {
            let supported =
                magi_model::ThinkingLevel::available(card.reasons, card.capabilities.as_ref());
            wants.thinking = Some(clamp_thinking(parsed, &supported));
        }
        wants.max_tokens_automatic |= wants.max_tokens.is_none();
        wants.max_tokens = Some(reserved(wants.max_tokens, card.max_output));
        let tool_filter = self.active_profile_ref().map_or_else(
            || self.tool_filter.clone(),
            crate::profiles::Profile::effective_tool_filter,
        );
        let profile_grants = self
            .active_profile_ref()
            .map_or_else(|| self.profile_grants.clone(), |p| p.allow.clone());
        card.ready.then(|| Backend {
            tools: self.tools.clone(),
            clients: self.clients.clone(),
            environ: self.environ.clone(),
            cwd: self.cwd.clone(),
            model: card.id.clone(),
            mind: self.mind.clone(),
            tooling: self.effective_tooling(),
            wants,
            context_window: card.context_window,
            max_output: card.max_output,
            session_identity: card
                .capabilities
                .as_ref()
                .and_then(|caps| caps.session_identity)
                == Some(true),
            transcript: self.transcript.clone(),
            image_resize: card
                .vision
                .then(|| {
                    card.capabilities
                        .as_ref()
                        .and_then(|cap| cap.input_limits.as_ref())
                        .and_then(|limits| limits.images.as_ref())
                        .and_then(|images| images.resize.clone())
                })
                .flatten(),
            system: self.effective_system(),
            confine: self.confine,
            isolate: self.isolate,
            grants: self.grants.clone(),
            profile_grants,
            tool_filter,
            deciders: self
                .cards
                .iter()
                .filter(|card| card.api == "decisions")
                .map(|card| card.id.clone())
                .collect(),
            helpers: self.effective_helpers(),
        })
    }

    /// Why a named model cannot be used. Apart from `backend` because "no such model" and "no key
    /// set for it" send a person to two different places.
    #[must_use]
    pub fn unusable(&self, name: &str) -> Option<String> {
        let card = self.find(name)?;
        (!card.ready).then(|| {
            format!(
                "{name} is offered by {}: {}",
                card.provider,
                setup::requirement(card)
            )
        })
    }

    /// Every model in the catalog, ready or not, with what it would take. All of them, because the
    /// person asking has usually configured nothing.
    #[must_use]
    pub fn choices(&self) -> Vec<magi_proto::ModelChoice> {
        let mut out: Vec<magi_proto::ModelChoice> = self
            .cards
            .iter()
            .map(|card| {
                let subscription = setup::is_subscription(card);
                magi_proto::ModelChoice {
                    vision: card.vision,
                    name: card.id.clone(),
                    provider: (!card.provider.is_empty()).then(|| card.provider.clone()),
                    display_name: card.display_name.clone(),
                    context_window: card.context_window.unwrap_or(0),
                    pricing: (!subscription).then(|| card.pricing.clone()).flatten(),
                    popularity_rank: (!subscription).then_some(card.popularity_rank).flatten(),
                    capabilities: card.capabilities.clone(),
                    requirement: setup::requirement(card),
                    wants_vars: setup::wanted_variables(card),
                    reasoning: card.reasons,
                    subscription,
                }
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

    #[tokio::test]
    async fn display_names_do_not_change_routing_or_create_aliases() {
        let mut held = Catalog::empty();
        let legacy =
            serde_json::json!({"id":"p/m","provider":"p","name":"m","api":"x","ready":true});
        let mut model: Card = serde_json::from_value(legacy).expect("legacy card");
        assert!(model.display_name.is_none());
        assert!(
            serde_json::to_value(&model)
                .expect("card")
                .get("display_name")
                .is_none()
        );
        model.display_name = Some("training-enabled: Model M".into());
        held.cards.push(model.clone());
        for route in ["m", "p/m"] {
            assert_eq!(held.backend(route).expect("route").model, "p/m");
        }
        assert!(held.backend("training-enabled: Model M").is_none());
        let choices = held.choices();
        assert_eq!(choices[0].name, "p/m");
        assert_eq!(choices[0].display_name, model.display_name);
        let bytes = serde_json::to_vec(&choices).expect("JSON");
        let restored: Vec<magi_proto::ModelChoice> = serde_json::from_slice(&bytes).expect("JSON");
        assert_eq!(restored, choices);
        let (write, read) = tokio::io::duplex(4096);
        let mut writer = magi_ipc::FrameWriter::new(write);
        let mut reader = magi_ipc::FrameReader::new(read);
        writer.write(&model).await.expect("CBOR card");
        assert_eq!(reader.read::<Card>().await.expect("CBOR card"), model);
        writer.write(&choices).await.expect("CBOR choices");
        assert_eq!(
            reader
                .read::<Vec<magi_proto::ModelChoice>>()
                .await
                .expect("CBOR choices"),
            choices
        );
    }

    pub(super) fn card(id: &str, name: &str, provider: &str, ready: bool) -> Card {
        Card {
            id: id.to_owned(),
            provider: provider.to_owned(),
            name: name.to_owned(),
            display_name: None,
            api: "openai-completions".to_owned(),
            context_window: Some(1000),
            max_output: Some(100),
            pricing: None,
            popularity_rank: None,
            capabilities: None,
            vision: false,
            reasons: false,
            ready,
            setup: None,
            needs: if ready {
                None
            } else {
                Some("MAGI_TEST_NOT_SET".to_owned())
            },
        }
    }

    pub(super) fn catalog() -> Catalog {
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
    fn choices_preserve_capability_metadata() {
        let mut catalog = catalog();
        catalog.cards[0].capabilities = Some(magi_model::Capabilities {
            session_identity: None,
            input: vec!["text".into(), "image".into()],
            output: vec!["text".into()],
            thinking: Some(vec!["low".into(), "high".into()]),
            thinking_levels: None,
            strict_tools: None,
            input_limits: None,
        });
        let choice = catalog
            .choices()
            .into_iter()
            .find(|choice| choice.name == "local/a")
            .expect("local model");
        assert_eq!(choice.capabilities, catalog.cards[0].capabilities);
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
    fn oauth_subscription_cards_drop_per_token_pricing_and_keep_provider() {
        let mut model = card("sub/pro", "pro", "sub", true);
        model.pricing = Some(magi_proto::ask::Pricing {
            input: Some("3".into()),
            output: Some("15".into()),
        });
        model.popularity_rank = Some(2);
        model.setup = Some(magi_proto::ask::Setup {
            state: magi_proto::ask::SetupState::Ready,
            methods: vec!["oauth".into()],
            variables: Vec::new(),
            fields: Vec::new(),
            login: true,
        });
        let held = Catalog {
            cards: vec![model],
            ..Catalog::empty()
        };
        let choices = held.choices();
        assert_eq!(choices[0].provider.as_deref(), Some("sub"));
        assert!(choices[0].subscription);
        assert!(choices[0].pricing.is_none());
        assert!(choices[0].popularity_rank.is_none());
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

/// What a backend reserves for its reply, and what it asks the provider to generate.
#[cfg(test)]
mod mem_output_reserve_matches_request {
    use super::tests::*;
    use super::*;

    #[test]
    fn a_reply_is_never_reserved_beyond_what_the_model_can_say() {
        // A 32k-window model that answers in at most 4096: reserving the 32k default left no room
        // at all, so the conversation was dropped from the first turn.
        assert_eq!(reserved(None, Some(4096)), 4096);
        assert_eq!(reserved(Some(64_000), Some(4096)), 4096);
        assert_eq!(reserved(Some(1000), Some(4096)), 1000);
        assert_eq!(reserved(None, None), REPLY);
        assert_eq!(reserved(None, Some(0)), REPLY);
    }

    #[test]
    fn the_reserve_is_the_limit_the_request_carries() {
        // Reserving room for an answer while naming no limit describes nothing: the provider
        // generates to its own default and the room held back is a guess about it.
        let backend = catalog().backend("local/a").expect("a backend");
        let reserve = reserved(backend.wants.max_tokens, backend.max_output);
        assert_eq!(
            backend.wants.max_tokens,
            Some(reserve),
            "what is asked for is what was reserved"
        );
    }

    #[test]
    fn it_is_never_more_than_the_model_can_say() {
        let backend = catalog().backend("local/a").expect("a backend");
        assert_eq!(backend.max_output, Some(100));
        assert_eq!(backend.wants.max_tokens, Some(100));
        assert!(backend.wants.max_tokens_automatic);
    }

    #[test]
    fn a_configured_limit_is_kept_when_the_model_can_meet_it() {
        let held = Catalog {
            wants: magi_proto::ask::Wants {
                max_tokens: Some(64),
                ..magi_proto::ask::Wants::default()
            },
            ..catalog()
        };
        assert_eq!(
            held.backend("local/a").expect("a backend").wants.max_tokens,
            Some(64)
        );
        assert!(
            !held
                .backend("local/a")
                .expect("a backend")
                .wants
                .max_tokens_automatic
        );
    }
}
