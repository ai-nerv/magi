-- AN EXAMPLE LUA PEER: tools written in Lua, running in their own process.
--
-- Not registered by default. To use it, add a tool declaration pointing at this file — the
-- path must be absolute or relative to the session directory, because that is where a peer is
-- started:
--
--     axum.tool("greet", {
--       transport = {
--         kind = "process",
--         command = axum.self,
--         args = { "ext", "lua", "/absolute/path/to/greet.lua" },
--       },
--     })
--
-- `axum.self` and not `"axum"`: this example said the latter for a milestone, which is the
-- exact bug M4 spent a live session finding. Naming the binary and trusting `PATH` finds
-- whichever copy the shell sees, and an older one that does not know `ext` fails as a broken
-- pipe with nothing to read -- a failure that looks like the peer, not like the path.
--
-- Note what that declaration does NOT contain: a description or a schema. This file is the
-- only description of the tool, and a peer declares itself when it connects. Writing them in
-- both places would be two descriptions of one tool, and the day they disagree the model is
-- handed the one that is wrong.
--
-- WHY A PEER RATHER THAN A LUA TOOL. `hexe.lua` and `oslo.lua` in `tools/` are Lua tools that
-- run *inside* the daemon: no serialisation, no process, and no isolation — a loop that never
-- ends takes the daemon with it. This is the same language on the other side of the wire.
--
--   * A tool that hangs, crashes or eats memory costs you the peer, not the session.
--   * It cannot be interrupted. A Lua body runs to completion inside the VM, so `esc` is
--     answered by the host killing the peer rather than by the peer stopping. State is lost.
--
-- Everything a Lua tool can normally reach is here — JSON, sockets, directory listing — and
-- everything it normally cannot is still gone: no `os.execute`, no `io`. Being in another
-- process is isolation, not permission. If it needs a shell, it is a shell peer.
--
-- One file may register several tools; the peer declares each of them on connect.

axum.tool("greet", {
  description = [[
Say hello to somebody, from a tool running in its own process.

An example. Copy this file, register what you actually want, and point a `transport` at it.]],

  parameters = {
    type = "object",
    properties = {
      who = { type = "string", description = "Who to greet." },
    },
    required = { "who" },
  },

  run = function(args)
    if type(args.who) ~= "string" or args.who == "" then
      return { content = "who must be a non-empty string", is_error = true }
    end
    return "hello " .. args.who
  end,
})
