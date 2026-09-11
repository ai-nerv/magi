//! The Lua VM magi runs on, and the config API it offers `init.lua`. See `EXTENDING.md`.
//!
//! Registration style: settings are assigned, behaviour registered, and the file returns nothing.
//! A registrar takes its id separately, so re-running it replaces. luna's values carry a collector
//! lifetime and never leave `lua.enter`.

pub mod acknowledged;
pub mod client;
mod convert;
mod engine;
mod fs;
mod json;
pub mod peer;
pub mod plugins;
mod sandbox;
mod stream;
pub mod tool;

pub use convert::{FromLua, json_from_lua};
pub use engine::{Config, Engine, balthasar_at, name_roles, name_session, roles, session};

#[derive(Debug, thiserror::Error)]
pub enum LuaError {
    #[error("{file}: {message}")]
    Syntax { file: String, message: String },

    #[error("{file}: {message}")]
    Runtime { file: String, message: String },

    #[error("{what}: {message}")]
    Shape { what: String, message: String },

    #[error("reading {file}: {source}")]
    Io {
        file: String,
        source: std::io::Error,
    },
}

/// Where a config lives, in the order applied; later files win, a project file last.
#[must_use]
pub fn search_paths() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    {
        out.push(config.join("magi").join("init.lua"));
    }
    out.push(std::path::PathBuf::from(".magi.lua"));
    out
}

#[cfg(test)]
mod tests;
