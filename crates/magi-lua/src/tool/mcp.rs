//! An MCP server, declared in a config and reached through the registry.
//!
//! Split from [`super`] under THE RULE.
//!
//! **The one interoperability gap that mattered.** Everything else this family does is its own
//! wire on purpose; tools are the exception, because the ecosystem settled and a harness that
//! cannot run an MCP server is one somebody has to leave to use what other people wrote.
//!
//! The design claim under test is that nothing else changed: a config declares a transport like
//! any other, the server's own names appear in the registry, and the registry checks a call
//! against the schema the *server* published.

use super::tests::built;
use magi_tools::ops::Real;

/// An MCP server declared in a config puts its tools in the registry.
///
/// **The one interoperability gap that mattered.** Everything else this family does is its
/// own wire on purpose; tools are the exception, because the ecosystem settled and a harness
/// that cannot run an MCP server is one somebody has to leave to use what others wrote.
///
/// The design claim under test is that nothing else changed: the config declares a transport
/// like any other, the server's own names appear in the registry, and the registry checks a
/// call against the schema the *server* published.
#[test]
fn a_config_may_declare_an_mcp_server_and_its_tools_appear() {
    use magi_model::scratch::Scratch;

    let dir = Scratch::new("magi-mcp-lua", "declared");
    let at = dir.join("server.py");
    std::fs::write(
        &at,
        r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    req = json.loads(line)
    m, i = req.get("method"), req.get("id")
    if i is None: continue
    if m == "initialize":
        out = {"protocolVersion": "2025-06-18"}
    elif m == "tools/list":
        out = {"tools": [{"name": "weather", "description": "The weather somewhere.",
                          "inputSchema": {"type": "object",
                                          "properties": {"where": {"type": "string"}},
                                          "required": ["where"]}}]}
    elif m == "tools/call":
        where = req.get("params", {}).get("arguments", {}).get("where", "")
        out = {"content": [{"type": "text", "text": "raining in " + where}]}
    else:
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": i, "result": out}), flush=True)
"#,
    )
    .expect("write");

    let (registry, _engine) = built(&format!(
        r#"
        magi.tool("weather-server", {{
          description = "an MCP server",
          parameters = {{ type = "object" }},
          transport = {{ kind = "mcp", command = "python3", args = {{ {:?} }} }},
        }})
        "#,
        at.display().to_string()
    ));

    // The server's own name, not the declaration's: MCP servers publish a list, and what the
    // model calls is what the server called it.
    assert!(registry.get("weather").is_some(), "the server's tool");
    assert!(
        registry.get("weather-server").is_none(),
        "the declaration names the server, not a tool"
    );

    let out = registry.call(
        "weather",
        &serde_json::json!({ "where": "Ghent" }),
        &Real::new(std::env::temp_dir()),
        &magi_tools::Uncancelled,
    );
    assert!(!out.is_error, "{out:?}");
    assert!(out.content.contains("raining in Ghent"), "{}", out.content);
}
