//! The copies of `config/` the binary carries.
//!
//! So a machine with no `~/.config/axum` still has a catalog, protocols and tools, and so
//! overriding one file means overriding one file.

/// Every file `config/` ships, in the order `init.lua` names them.
///
/// The binary carries the whole tree so a machine with no `~/.config/axum` still has a catalog,
/// protocols and tools. `axum.load("apis/google.lua")` finds the installed copy if there is one
/// and this otherwise, which is what makes overriding one file mean overriding one file. The
/// order is also the fallback set for an entry point that names nothing, so stubs come before
/// the tools that open them.
pub(super) const SHIPPED: &[(&str, &str)] = &[
    (
        "stubs/axum.lua",
        include_str!("../../../../config/stubs/axum.lua"),
    ),
    (
        "stubs/hexe.lua",
        include_str!("../../../../config/stubs/hexe.lua"),
    ),
    (
        "stubs/oslo.lua",
        include_str!("../../../../config/stubs/oslo.lua"),
    ),
    (
        "apis/openai-completions.lua",
        include_str!("../../../../config/apis/openai-completions.lua"),
    ),
    (
        "apis/openai-responses.lua",
        include_str!("../../../../config/apis/openai-responses.lua"),
    ),
    (
        "apis/anthropic-messages.lua",
        include_str!("../../../../config/apis/anthropic-messages.lua"),
    ),
    (
        "apis/google.lua",
        include_str!("../../../../config/apis/google.lua"),
    ),
    (
        "apis/pi-messages.lua",
        include_str!("../../../../config/apis/pi-messages.lua"),
    ),
    (
        "providers.lua",
        include_str!("../../../../config/providers.lua"),
    ),
    ("system.lua", include_str!("../../../../config/system.lua")),
    (
        "tools/shell.lua",
        include_str!("../../../../config/tools/shell.lua"),
    ),
    (
        "tools/hexe.lua",
        include_str!("../../../../config/tools/hexe.lua"),
    ),
    (
        "tools/oslo.lua",
        include_str!("../../../../config/tools/oslo.lua"),
    ),
];

/// The entry point, and the only file the host runs by name.
pub(super) const SHIPPED_INIT: &str = include_str!("../../../../config/init.lua");

/// The source of one shipped path, if it is one.
pub(super) fn shipped(path: &str) -> Option<&'static str> {
    SHIPPED
        .iter()
        .find_map(|(name, source)| (*name == path).then_some(*source))
}
