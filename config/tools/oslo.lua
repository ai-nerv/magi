-- `oslo`, as a LUA tool.
--
-- The same branch as `hexe` and for the same reason: the shell is already running and answers
-- over a socket, so this is a function rather than a process.
--
-- Note what it is *not*: this does not run commands. Asking oslo what it knows is a different
-- thing from asking it to do something, and the second is what `shell` is for — behind a
-- process boundary, where it belongs.

-- The stub arrives as source in `axum.stubs`, handed in by the host. A config cannot open
-- files -- `io` is not reachable -- and needing one stub is no reason to change that.
local function client()
  local source = axum.stubs and axum.stubs.oslo
  if not source then return nil, "oslo's client stub is not installed; run `make configs`" end
  local chunk, why = load(source, "oslo.lua")
  if not chunk then return nil, why end
  return chunk(axum.stream)
end


-- Discovery is the stub's. It was hand-rolled here because the stub could not list a directory
-- from inside axum -- its list of hosts to ask named `oslo` and `hexe` and not the one it was
-- running in -- and the workaround guessed a layout: `$XDG_RUNTIME_DIR/oslo/api@*.sock`, which is
-- hexe's shape, not oslo's. oslo puts its sockets in `$XDG_RUNTIME_DIR/onix/oslo/<id>.sock`, so
-- this tool had never once connected. `axum` is in the stub's host list now, so the stub's own
-- discovery works here, and it knows where oslo keeps its sockets because it is oslo's file.


axum.tool("oslo", {
  description = [[
Ask the oslo shell about its own state: environment, working directory, and what it can do.

Reads only. To run a command, use `shell`.]],

  parameters = {
    type = "object",
    properties = {
      what = {
        type = "string",
        description = "Which verb to ask. `verbs` lists what this shell offers.",
      },
    },
  },

  transport = { kind = "lua" },

  run = function(args)
    local oslo, why = client()
    if not oslo then return { content = tostring(why), is_error = true } end

    local shell, refused = oslo.connect()
    if not shell then
      return { content = "no oslo session is running (" .. tostring(refused) .. ")" }
    end

    local what = args.what or "verbs"
    local ok, answer = pcall(function() return shell[what]() end)
    shell:close()
    if not ok then
      return { content = "oslo refused " .. what .. ": " .. tostring(answer), is_error = true }
    end
    return { content = axum.json.encode(answer) }
  end,
})
