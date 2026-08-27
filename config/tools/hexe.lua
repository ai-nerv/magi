-- `hexe`, as a LUA tool.
--
-- The other branch. This one needs no process of its own: it asks a multiplexer that is
-- already running, over the socket the family shares. The VM already has what that takes —
-- `axum.stream` to open the socket and hexe's own client stub to speak its protocol — so the
-- whole tool is a function.
--
-- That is the distinction the two branches are for. `bash` needs isolation and a life of its
-- own; this needs a socket and twenty lines. Making both a process would waste one; making
-- both Lua would give the config layer a shell.

-- The stub arrives as source in `axum.stubs`, handed in by the host. A config cannot open
-- files -- `io` is not reachable -- and needing one stub is no reason to change that.
local function client()
  local source = axum.stubs and axum.stubs.hexe
  if not source then return nil, "hexe's client stub is not installed; run `make configs`" end
  local chunk, why = load(source, "hexe.lua")
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

axum.tool("hexe", {
  description = [[
Inspect the terminal multiplexer this session is running under: which panes and tabs exist,
what is running in each, and where they are rooted.

Use it to find out what the user is looking at. It reads; it does not rearrange anything.]],

  parameters = {
    type = "object",
    properties = {
      what = {
        type = "string",
        enum = { "panes", "tabs", "session", "verbs" },
        description = "Which question to ask. Defaults to panes.",
      },
    },
  },

  transport = { kind = "lua" },

  run = function(args)
    local hexe, why = client()
    if not hexe then return { content = tostring(why), is_error = true } end

    local where = socket("hexe")
    local mux, refused = hexe.connect(where and { path = where } or nil)
    if not mux then
      -- Not an error the model should work around: there is simply no mux here.
      return { content = "no hexe session is running (" .. tostring(refused) .. ")" }
    end

    local what = args.what or "panes"
    local ok, answer = pcall(function() return mux[what]() end)
    mux:close()
    if not ok then
      return { content = "hexe refused " .. what .. ": " .. tostring(answer), is_error = true }
    end
    return { content = axum.json.encode(answer) }
  end,
})
