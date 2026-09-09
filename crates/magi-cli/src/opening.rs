//! Everything a session settles before it opens.
//!
//! Three things happen here and the order between them is the whole reason the module exists:
//! the configuration is read, this session's own socket is named, and melchior is started and
//! names the session. The name is what everything downstream is filed under — the run balthasar
//! keeps a history in, and the agent it keeps scratch for — so it has to be settled before the
//! balthasar that will hold them is spawned, not somewhere in the middle of opening a session.
//!
//! # The socket is named here, and bound much later
//!
//! It used to be made in `run`, a moment before [`crate::host::start`] bound it — which is the
//! natural place for it, and it was the wrong one as soon as melchior had to be *told* the path.
//! melchior is started here, in the prologue, and `serve --ui` is read at announce time: a path
//! settled after that would reach the directory a second or two late at best, and the roster
//! every peer reads is pushed on a tick, so the window is one in which siblings have already
//! seen this agent listed with no screen.
//!
//! Nothing was rearranged to make that work. [`crate::session::key`] is a pid and a clock — it
//! asks nothing of the config, the layer or the store — so naming the socket is a thing this
//! process could always have done first, and moving it here makes the path *one field* rather
//! than two `unwrap_or_else` calls that have to keep agreeing. The one published is the one
//! bound because they are now the same value, not because two places compute it the same way.
//!
//! Nothing here is async and nothing here needs to be: reading a config is a file, and melchior
//! answers the line that names this session before it does anything else.

/// What a session knows about itself before it opens.
pub struct Opening {
    /// The configuration, read once, for everything downstream that needs it.
    pub loaded: Option<crate::config::Loaded>,
    /// This project's name.
    pub project: String,
    /// What this session's own files are named after — see [`crate::session::key`].
    pub key: String,
    /// Where this session will bind the socket its UI talks to it over.
    ///
    /// Named before melchior is started, because melchior publishes it and cannot be told later
    /// without the roster being wrong in between.
    pub socket: std::path::PathBuf,
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
    ///
    /// `socket` is what a caller asked for on the command line, which is nobody in an ordinary
    /// session: `--socket` exists for the replay host and for pointing a UI at something by
    /// hand. Taken here rather than in `run` so that the path melchior publishes and the path
    /// this process binds cannot be two different answers.
    ///
    /// `role` is what the command line said this session is for, and it is settled here for the
    /// same reason the socket is: melchior writes the role into the directory at announce time,
    /// so one arriving later leaves a window in which the agent is on every peer's roster
    /// described as `main`. A headless magi is the case that has no other source — nobody minted
    /// it, so there is no inherited role to fall back on.
    #[must_use]
    pub fn begin(socket: Option<std::path::PathBuf>, role: crate::melchior::Role<'_>) -> Self {
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
        // Named before the layer is started, so `serve --ui` has something to announce. The key
        // is a pid and a clock and depends on nothing here — see the module note.
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
            key,
            socket,
            named,
            started,
            run,
            agent,
        }
    }
}
