-- `oslo`, as a LUA tool.
--
-- The same branch as `hexe` and for the same reason: the shell is already running and answers
-- over a socket, so this is a function rather than a process.
--
-- Note what it is *not*: this does not run commands. Asking oslo what it knows is a different
-- thing from asking it to do something, and the second is what `bash` is for — behind a
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


-- Find the sibling's newest control socket.
--
-- Done here rather than left to the stub. A stub looks for the family's globals to find a host
-- that can list a directory, and its list names the siblings that existed when it was written --
-- inside axum it finds neither `_G.hexe` nor `_G.oslo` and falls through to shelling out, which
-- a config cannot do. Handing over a path sidesteps a question it should not have to answer.
local function socket(name)
  local runtime = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
  local dir = runtime .. "/" .. name
  local newest, when = nil, -1
  for _, entry in ipairs(axum.fs.ls(dir) or {}) do
    if entry.name:sub(1, 4) == "api@" and entry.name:sub(-5) == ".sock" then
      -- Newest first: a socket left by a frontend that was killed looks exactly like a live
      -- one until something connects to it.
      if (entry.mtime or 0) > when then
        newest, when = dir .. "/" .. entry.name, entry.mtime or 0
      end
    end
  end
  return newest
end

axum.tool("oslo", {
  description = [[
Ask the oslo shell about its own state: environment, working directory, and what it can do.

Reads only. To run a command, use `bash`.]],

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

    local where = socket("oslo")
    local shell, refused = oslo.connect(where and { path = where } or nil)
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
