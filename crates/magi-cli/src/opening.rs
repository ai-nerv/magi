//! Everything a session settles before it opens: the configuration is read, this session's socket
//! is named, and melchior is started and names the session.

/// What a session knows about itself before it opens.
pub struct Opening {
    pub loaded: Option<crate::config::Loaded>,
    pub project: String,
    pub key: String,
    /// Named before melchior is started, because melchior publishes it and cannot be told later.
    pub socket: std::path::PathBuf,
    pub named: String,
    pub started: Option<(crate::melchior::Melchior, std::path::PathBuf)>,
    pub run: Option<String>,
    pub agent: Option<String>,
}

impl Opening {
    /// Read the configuration and start the layer, so this session has a name to be filed under.
    #[must_use]
    pub fn begin(socket: Option<std::path::PathBuf>, role: crate::melchior::Role<'_>) -> Self {
        // Loaded once: a second `load` re-runs every configuration file and repeats every refusal.
        let loaded = crate::config::load().ok();
        let project =
            crate::session::project(loaded.as_ref().and_then(|l| l.config.string("project")));

        // melchior first, because it names this session; absent, this session has no siblings.
        let program = loaded.as_ref().map_or_else(
            || magi_host::broker::MELCHIOR.to_owned(),
            crate::config::mind,
        );
        let key = crate::session::key();
        let socket = socket.unwrap_or_else(|| crate::session::socket_for(&project, &key));
        let started = crate::melchior::Melchior::start(
            &program,
            &project,
            crate::talk(loaded.as_ref()),
            &socket,
            role,
        );
        let named = started
            .as_ref()
            .map(|(melchior, _)| melchior.named.clone())
            .unwrap_or_default();
        // What melchior said, and only otherwise what magi can work out: a run derived here would
        // name one no other agent of it reads off the directory.
        let run = started
            .as_ref()
            .map(|(melchior, _)| melchior.run.clone())
            .filter(|run| !run.is_empty())
            .or_else(|| crate::melchior::run_of(&named));
        let agent = crate::balthasar::agent_of(&named).map(str::to_owned);

        Self {
            loaded,
            project,
            key,
            socket,
            named,
            started,
            run,
            agent,
        }
    }
}
