-- `bash`, as a PROCESS tool.
--
-- This is the branch that exists for tools like this one. Running shell commands is the most
-- dangerous thing the model can ask for, the thing you would most want to isolate, and the
-- thing that most needs to outlive a single call — `cd build` then `make` only works if one
-- process holds its own working directory between calls.
--
-- So it is not a built-in and it is not Lua. It is a peer in its own process, spoken to over
-- length-prefixed CBOR: any language, crash-isolated, and sandboxable later without touching
-- anything here. A Lua tool deliberately cannot do this — it has no shell, precisely so that
-- this boundary is the only way to get one.
--
-- Replace `command` to run commands somewhere else entirely — a container, another machine,
-- a shell with different credentials. Nothing above this line changes.
--
-- The description and schema below are a FALLBACK, not the truth. A peer declares what it
-- offers when it connects, and what it says wins: it is the only thing that knows what it
-- actually implements. These are here for the case where the peer cannot be reached at all --
-- a wrong command, a missing binary -- so the model is told something rather than being handed
-- a tool with no schema it can never call correctly.

axum.tool("bash", {
  description = [[
Run a shell command in the session's directory.

The working directory and environment persist between calls, so `cd` and exported variables
carry over. Output is returned as it is produced.]],

  parameters = {
    type = "object",
    properties = {
      command = { type = "string", description = "The command line to run." },
    },
    required = { "command" },
  },

  transport = {
    kind = "process",
    -- axum is a multi-call binary, so its own shell peer is the same executable under
    -- another name. A peer of your own goes here instead.
    command = "axum",
    args = { "ext", "shell" },
  },
})
