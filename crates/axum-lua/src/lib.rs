//! The Lua VM axum runs on, and the config API it offers `init.lua`.
//!
//! Same VM as oslo, pinned to the same tag: `luna`, stackless, pure Rust, tracing collector.
//! No C anywhere, so the static musl build still needs nothing installed.
//!
//! # The shape of the config
//!
//! Registration style, as the family uses it. Settings are assigned, behaviour is registered,
//! and the file returns nothing:
//!
//! ```lua
//! local axum = require("axum")
//!
//! axum.model = "anthropic/claude-sonnet-4-5"   -- a setting, assigned
//! axum.scan_speed = 2                          -- another one
//!
//! axum.provider("my-vllm", {                    -- a description, handed in
//!   name = "My vLLM box",
//!   api = "openai-completions",
//!   base_url = "http://10.0.0.7:8000/v1",
//!   models = { { id = "Qwen/Qwen3-Coder-30B", context_window = 262144 } },
//! })
//!
//! axum.on.session(function(s) ... end)          -- behaviour, registered, repeatable
//! ```
//!
//! A provider is a **table handed to a registrar**, never a fragment the config assembles by
//! hand. That is why `axum.provider(id, spec)` takes the id separately: the registration has an
//! identity, so re-running it replaces rather than appends, and a config that loops over a
//! directory of machines is idempotent.
//!
//! # The lifetime that shapes everything
//!
//! luna's values carry a collector lifetime — `Value<'gc>` exists only inside
//! `lua.enter(|ctx| …)`. Nothing outside this crate ever sees one: the config is evaluated,
//! what it declared is converted to owned Rust values at the boundary, and the VM's values do
//! not escape. That keeps the lifetime out of the twelve crates that have nothing to do with
//! Lua.

pub mod adapter;
mod convert;
mod engine;
mod fs;
mod json;
pub mod peer;
mod sandbox;
mod stream;
pub mod stub;
pub mod tool;

pub use convert::{FromLua, json_from_lua};
pub use engine::{Config, Engine, Registered};

/// Anything that can go wrong loading a config.
#[derive(Debug, thiserror::Error)]
pub enum LuaError {
    /// The chunk would not compile.
    ///
    /// Fatal, and it names the file and line: a config that does not parse has not expressed an
    /// intention, so guessing at one is worse than stopping.
    #[error("{file}: {message}")]
    Syntax {
        /// The file that would not compile.
        file: String,
        /// What the parser said.
        message: String,
    },

    /// The chunk compiled and raised while running.
    #[error("{file}: {message}")]
    Runtime {
        /// The file that raised.
        file: String,
        /// What was raised.
        message: String,
    },

    /// A declaration was the wrong shape.
    #[error("{what}: {message}")]
    Shape {
        /// What was being read.
        what: String,
        /// Why it did not fit.
        message: String,
    },

    /// The file could not be read.
    #[error("reading {file}: {source}")]
    Io {
        /// The file that failed.
        file: String,
        /// Why.
        source: std::io::Error,
    },
}

/// Where a config lives, in the order the files are applied.
///
/// Later files win, and a project file is last so a repository can override a machine.
#[must_use]
pub fn search_paths() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    {
        out.push(config.join("axum").join("init.lua"));
    }
    out.push(std::path::PathBuf::from(".axum.lua"));
    out
}

#[cfg(test)]
mod tests;
