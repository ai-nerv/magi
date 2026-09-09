//! Everything a session settles before it opens.
//!
//! Two things happen here and the order between them is the whole reason the module exists: the
//! configuration is read, and melchior is started and names this session. The name is what
//! everything downstream is filed under — the run balthasar keeps a history in, and the agent it
//! keeps scratch for — so it has to be settled before the balthasar that will hold them is
//! spawned, not somewhere in the middle of opening a session.
//!
//! Nothing here is async and nothing here needs to be: reading a config is a file, and melchior
//! answers the line that names this session before it does anything else.

/// What a session knows about itself before it opens.
pub struct Opening {
    /// The configuration, read once, for everything downstream that needs it.
    pub loaded: Option<crate::config::Loaded>,
    /// This project's name.
    pub project: String,
    /// `project/role/id` as melchior gave it, or empty when melchior is not installed.
    pub named: String,
    /// The running layer and the socket it bound, or `None` for a session without siblings.
    pub started: Option<(crate::melchior::Melchior, std::path::PathBuf)>,
    /// Which run this session belongs to, which is what balthasar files its history under.
    pub run: Option<String>,
    /// Which agent of that run this is, which balthasar files its scratch under.
    pub agent: Option<String>,
}

impl Opening {
    /// Read the configuration and start the layer, so this session has a name to be filed under.
    #[must_use]
    pub fn begin() -> Self {
        // Loaded once, here. Every later reader is handed this one: a second `load` in the same
        // process runs every configuration file again and repeats every refusal it printed the
        // first time.
        let loaded = crate::config::load().ok();
        let project =
            crate::session::project(loaded.as_ref().and_then(|l| l.config.string("project")));

        // melchior first, because it names this session and the name is what everything after
        // this is filed under. Absent, this is a session with no siblings and no `agent` tool —
        // and otherwise a working session, which is the whole point of the layer being a
        // separate program.
        let program = loaded.as_ref().map_or_else(
            || magi_host::broker::MELCHIOR.to_owned(),
            crate::config::mind,
        );
        let started =
            crate::melchior::Melchior::start(&program, &project, crate::talk(loaded.as_ref()));
        let named = started
            .as_ref()
            .map(|(melchior, _)| melchior.named.clone())
            .unwrap_or_default();
        // What melchior said, and only otherwise what magi can work out. A root's run is its id
        // and the moment it began, so deriving one here would name a run no other agent of it
        // reads off the directory — and two agents that cannot agree on the run are two agents
        // filing their memory where the other will not look.
        let run = started
            .as_ref()
            .map(|(melchior, _)| melchior.run.clone())
            .filter(|run| !run.is_empty())
            .or_else(|| crate::melchior::run_of(&named));
        let agent = crate::balthasar::agent_of(&named).map(str::to_owned);

        Self {
            loaded,
            project,
            named,
            started,
            run,
            agent,
        }
    }
}
