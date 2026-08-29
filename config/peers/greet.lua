-- AN EXAMPLE LUA PEER: a tool written in Lua, running in its own process.
-- Not registered by default. Point a declaration at it, and note that a peer declares its own
-- description and schema on connect, so the tool table carries only the transport:
--
--     axum.tool("greet", {
--       transport = {
--         kind = "process",
--         command = axum.self,
--         args = { "ext", "lua", "/absolute/path/to/greet.lua" },
--       },
--     })
--
-- A peer rather than a Lua tool because a Lua tool runs inside the daemon: no isolation, and
-- no interrupt. Being in another process is isolation, not permission — `os.execute` and `io`
-- are gone here too.


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
