-- `hexe`, as a Lua tool: it asks a multiplexer that is already running, over the socket the
-- family shares. No process of its own, so the whole tool is a function.

-- The stub arrives as source in `axum.stubs`: a config cannot open files.
local function client()
  local source = axum.stubs and axum.stubs.hexe
  if not source then return nil, "hexe's client stub is not installed; run `make configs`" end
  local chunk, why = load(source, "hexe.lua")
  if not chunk then return nil, why end
  return chunk(axum.stream)
end


-- Discovery is the stub's, for the same reason it is oslo's: `axum` is in the stub's host list
-- now, so it can list the socket directory from in here rather than shelling out to a VM that
-- refuses. What was hand-rolled here happened to match hexe's layout and so happened to work --


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

    local mux, refused = hexe.connect()
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
