-- `oslo`, as a Lua tool, for the same reason `hexe` is one: the shell is already running and
-- answers over a socket. It does not run commands — that is what `shell` is for.

-- The stub arrives as source in `axum.stubs`: a config cannot open files.
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
