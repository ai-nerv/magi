-- A tool that wraps a real program.
--
-- Copy this into `~/.config/magi/plugin/` and it is in the next session -- no edit to `init.lua`,
-- no rebuild, nothing else to tell.
--
-- What a tool declaration owes:
--
--   name          what the model calls it. Registering the same name twice replaces, so a file in
--                 `after/plugin/` can override anything a package declared.
--   description   what it does, in the model's terms. This is the whole of what the model knows.
--   parameters    JSON Schema for the arguments. The model is held to it before you see them.
--   transport     how the body is reached. `{ kind = "lua" }` for a `run` function like this one.
--   needs         which permission verb this acts under -- `read`, `write`, `run` or `reach`.
--                 Omit it for a tool that touches nothing a person would want a say over.
--   run           the body. Return `{ content = "..." }`, or `{ content = ..., is_error = true }`.
--
-- `magi.shell` is the seam every tool runs commands through, so this is gated exactly as the
-- built-in shell is: the person is asked, the answer is remembered, and `magi.confine` applies.
-- The VM itself has no `os.execute` and no `io.popen` -- see the sandbox.

magi.tool("ripgrep", {
  description = "Search the working tree for a regular expression. Faster than grep and "
    .. "respects .gitignore. Returns matching lines with their file and line number.",

  -- **How the body is reached.** `{ kind = "lua" }` means the `run` below, in this VM. The other
  -- kinds are declarations rather than code: `command` spawns one program per call with the
  -- arguments in argv, and `casper` hands the call to casper. A tool with a `run` and no
  -- transport is refused at load with "missing field `transport`" -- the registry has no way to
  -- guess that the function is the point.
  transport = { kind = "lua" },

  needs = "run",
  parameters = {
    type = "object",
    properties = {
      pattern = { type = "string", description = "The regular expression to search for." },
      path = { type = "string", description = "Where to search. Defaults to the whole tree." },
    },
    required = { "pattern" },
  },
  run = function(args)
    -- Quoted, because the pattern comes from a model and a model will eventually send a quote.
    local function quoted(text)
      return "'" .. tostring(text):gsub("'", "'\\''") .. "'"
    end

    local command = "rg --line-number --no-heading --color=never -- "
      .. quoted(args.pattern)
      .. " "
      .. quoted(args.path or ".")

    -- Two returns, not a table: `stdout, err`. `err` is nil when the command succeeded, the
    -- program's stderr when it exited non-zero, and the refusal when the person said no -- in
    -- which case `stdout` is nil too.
    local out, err = magi.shell(command)
    if out == nil then
      return { content = err or "rg could not be run", is_error = true }
    end
    if err and out == "" then
      -- rg exits 1 with no output for "nothing matched", which is an answer rather than a
      -- failure -- so an empty result is reported as one and the model does not retry.
      return { content = "no matches" }
    end
    if err then
      return { content = err, is_error = true }
    end
    return { content = out }
  end,
})
