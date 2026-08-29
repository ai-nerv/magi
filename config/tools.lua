-- The tools axum ships.
--
-- `shell` runs in a peer process because running commands is the thing most worth isolating.
-- A tool of your own is either that or a Lua function declared here.

do -- shell
  axum.tool("shell", {
    description = [[
  Run a command in the user's own shell (`$SHELL`, falling back to `sh`).

  The working directory and environment persist between calls, so `cd` and exported variables
  carry over. Output is returned as it is produced.]],

    parameters = {
      type = "object",
      properties = {
        command = { type = "string", description = "The command line to run." },
        timeout = {
          type = "integer", minimum = 1, maximum = 600,
          description = "Seconds to allow before giving up. Defaults to 600.",
        },
      },
      required = { "command" },
    },

    transport = {
      kind = "process",
      -- axum is a multi-call binary, so its own shell peer is the same executable under
      -- another name. `axum.self` is the path of the binary that is running: naming it "axum"
      -- and hoping PATH agrees finds whichever copy the shell sees, and an older one fails as
      command = axum.self,
      args = { "ext", "shell" },
    },
  })
end

