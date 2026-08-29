-- The tools axum ships.
--
-- `shell` runs in a peer process because running commands is the thing most worth isolating;
-- the other two ask a sibling that is already running, over the socket the family shares, so
-- they are functions in this VM. A tool of your own goes in either camp.

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

do -- hexe
  -- The client arrives as source in `axum.clients`: a config cannot open files.
  local function client()
    local source = axum.clients and axum.clients.hexe
    if not source then return nil, "hexe's client library is not installed; run `make configs`" end
    local chunk, why = load(source, "hexe.lua")
    if not chunk then return nil, why end
    return chunk(axum.stream)
  end


  -- Discovery is the client's, for the same reason it is oslo's: `axum` is in the client's host list
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
end

do -- oslo
  -- The client arrives as source in `axum.clients`: a config cannot open files.
  local function client()
    local source = axum.clients and axum.clients.oslo
    if not source then return nil, "oslo's client library is not installed; run `make configs`" end
    local chunk, why = load(source, "oslo.lua")
    if not chunk then return nil, why end
    return chunk(axum.stream)
  end


  -- Discovery is the client's. It was hand-rolled here because the client could not list a directory
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
end
