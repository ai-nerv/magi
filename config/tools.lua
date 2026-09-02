-- The tools axon ships.
--
-- `shell` runs in a peer process because running commands is the thing most worth isolating;
-- the other two ask a sibling that is already running, over the socket the family shares, so
-- they are functions in this VM. A tool of your own goes in either camp.

do -- shell
  axon.tool("shell", {
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
      -- axon is a multi-call binary, so its own shell peer is the same executable under
      -- another name. `axon.self` is the path of the binary that is running: naming it "axon"
      -- and hoping PATH agrees finds whichever copy the shell sees, and an older one fails as
      command = axon.self,
      args = { "ext", "shell" },
    },
  })
end

do -- hexe
  -- The client arrives as source in `axon.clients`: a config cannot open files.
  local function client()
    local source = axon.clients and axon.clients.hexe
    if not source then return nil, "hexe's client library is not installed; run `make configs`" end
    local chunk, why = load(source, "hexe.lua")
    if not chunk then return nil, why end
    return chunk(axon.stream)
  end


  -- Discovery is the client's, for the same reason it is oslo's: `axon` is in the client's host list
  -- now, so it can list the socket directory from in here rather than shelling out to a VM that
  -- refuses. What was hand-rolled here happened to match hexe's layout and so happened to work --


  axon.tool("hexe", {
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
      return { content = axon.json.encode(answer) }
    end,
  })
end

do -- oslo
  -- The client arrives as source in `axon.clients`: a config cannot open files.
  local function client()
    local source = axon.clients and axon.clients.oslo
    if not source then return nil, "oslo's client library is not installed; run `make configs`" end
    local chunk, why = load(source, "oslo.lua")
    if not chunk then return nil, why end
    return chunk(axon.stream)
  end


  -- Discovery is the client's. It was hand-rolled here because the client could not list a directory
  -- from inside axon -- its list of hosts to ask named `oslo` and `hexe` and not the one it was
  -- running in -- and the workaround guessed a layout: `$XDG_RUNTIME_DIR/oslo/api@*.sock`, which is


  axon.tool("oslo", {
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
      return { content = axon.json.encode(answer) }
    end,
  })
end

do -- find
  axon.tool("find", {
    description = [[
  Find files and directories by name. Returns one path a line.

  Honours .gitignore. Results are capped; narrow the glob or the path rather than raising it.]],

    parameters = {
      type = "object",
      properties = {
        glob = { type = "string", description = "Name pattern, e.g. `*.rs` or `Cargo.toml`." },
        path = { type = "string", description = "Where to look. Defaults to the session's directory." },
        limit = {
          type = "integer", minimum = 1, maximum = 5000, default = 1000,
          description = "Most paths to return.",
        },
      },
    },

    -- `--glob` is a boolean and the pattern is positional, so an absent glob leaves the flag
    -- behind with nothing to match, which is fd's own way of saying "everything".
    transport = {
      kind = "command",
      command = "fd",
      args = {
        "--color=never", "--glob",
        "--max-results={limit}",
        "{glob}",
        "--search-path={path}",
      },
      timeout = 30,
    },
  })
end

do -- ls
  axon.tool("ls", {
    description = [[
  List a directory. Returns one entry a line, with a trailing `/` on directories.]],

    parameters = {
      type = "object",
      properties = {
        path = { type = "string", description = "The directory. Defaults to the session's directory." },
      },
    },

    transport = {
      kind = "command",
      command = "ls",
      args = { "-1", "-p", "-A", "--color=never", "{path}" },
      timeout = 10,
    },
  })
end

do -- grep
  -- What the `command` transport cannot express: choosing the program at call time. `rg` honours
  -- ignore files and is faster; `grep` is everywhere. Both go through the same gate.
  local function search(pattern, path, limit)
    local where = path or "."
    local out, err = axon.shell(
      ("rg --line-number --no-heading --color=never --max-count=%d --regexp=%q %q")
        :format(limit, pattern, where))
    if out and out ~= "" then return out end
    -- `rg` absent, or nothing matched. `grep` tells the two apart by trying.
    return axon.shell(
      ("grep -rnI --exclude-dir=.git --max-count=%d -e %q %q")
        :format(limit, pattern, where)) or (err or "no matches")
  end

  axon.tool("grep", {
    description = [[
  Search file contents, preferring ripgrep and falling back to grep.

  Honours .gitignore when ripgrep is available. Returns `path:line:text`, one match a line.]],

    parameters = {
      type = "object",
      properties = {
        pattern = { type = "string", description = "The pattern to search for." },
        path = { type = "string", description = "Where to search. Defaults to the session's directory." },
        limit = {
          type = "integer", minimum = 1, maximum = 500, default = 100,
          description = "Most matches to return.",
        },
      },
      required = { "pattern" },
    },

    transport = { kind = "lua" },

    run = function(args)
      local found = search(args.pattern, args.path, args.limit or 100)
      if not found or found == "" then return { content = "no matches" } end
      return { content = found }
    end,
  })
end

do -- memo
  -- The memory layer, if it is installed and running. memo publishes its own tool descriptors --
  -- `remember`, `recall`, `forget` -- so the vocabulary is written once, in memo, rather than
  -- copied here to drift.
  local function client()
    local source = axon.clients and axon.clients.memo
    if not source then return nil, "memo's client library is not installed" end
    local chunk, why = load(source, "memo.lua")
    if not chunk then return nil, why end
    return chunk(axon.stream)
  end

  -- Asked at load, because a tool has to exist before the model is told what it may call. memo
  -- being absent is the ordinary case, not an error: nothing is registered and the session runs
  -- without memory, which is what every session did before memo existed.
  local memo = select(1, client())

  -- The last context memo handed over. A recall that comes back with an injection id is memo
  -- saying "these went into your model's context, tell me what you did with them" -- and this
  -- is the only place that id is held, because nothing else in axon needs to know it exists.
  local injection = nil

  local asked, offered = pcall(function() return memo and memo.tools() end)
  if asked and offered then
    for _, t in ipairs(offered) do
      axon.tool(t.name, {
        description = t.description,
        parameters = t.parameters,
        transport = { kind = "lua" },
        run = function(args)
          local answer, why = memo.fetch({ tool = "memo" }, t.verb, args)
          if not answer then return { content = tostring(why), is_error = true } end
          -- Kept, and stripped from what the model sees. The id is bookkeeping between axon
          -- and memo; putting it in the context would spend tokens on a handle the model can
          -- do nothing with, and invite it to make one up.
          if type(answer) == "table" and answer.injection then
            injection = answer.injection
            answer = answer.memories or answer
          end
          return { content = axon.json.encode(answer) }
        end,
      })
    end
  end

  -- Close the loop. Every tool that finishes after memo handed something over is reported back:
  -- what ran, and whether it worked. memo decides for itself whether the action followed any of
  -- the memories it gave -- axon does not guess, because a harness claiming a match it did not
  -- verify is asserting an analysis rather than reporting an action.
  --
  -- Nothing here is required. With memo absent, or its ledger off, `injection` stays nil and
  -- this never fires; the session runs exactly as it did before.
  axon.watch("memo-outcome", {
    run = function(event)
      if not memo or not injection then return end
      if event.tool == "recall" or event.tool == "remember" then return end

      -- What the tool was actually asked to do, as one string. memo hashes it and keeps the
      -- digest; the arguments themselves never leave this VM.
      local args = event.arguments or {}
      local action = args.command or args.path or args.query or ""

      local used = memo.fetch({ tool = "memo" }, "used", injection, {
        tool = event.tool,
        action = action,
      })
      if not used or not used.action then return end

      memo.fetch({ tool = "memo" }, "outcome", used.action, {
        kind = event.is_error and "failed" or "succeeded",
      })
    end,
  })
end

do -- agent
  -- Talking to the other axons in this project, through atom -- a separate program that owns
  -- naming, the sockets sessions reach each other on, and the walls between them.
  --
  -- `atom` rather than `axon ext agent`, and a `command` rather than a `process`: this ran as
  -- axon's own peer until the layer left, and neither half of that is a rename. A command
  -- transport is one exec per call with the arguments in argv, which is the whole protocol atom
  -- offers -- deliberately, so a harness that can run a program can use it without copying
  -- anybody's message types.
  --
  -- Delete this block if atom is not installed. The tool then fails per call rather than at
  -- load, which is the honest outcome: a session with no atom has no siblings to talk to.
  axon.tool("agent", {
    description = [[
  Talk to the other axon instances running in this project: ask what they are doing, send them
  work, answer their questions, and stop the ones this session started.

  Call it with `verb: "help"` first -- that lists every verb and what each one takes, from the
  atom that is actually installed. `list` says who is really there.]],

    -- Five arguments and no list of verbs, on purpose. atom's vocabulary grows and this file
    -- would not hear about it; `help` is the copy that cannot go stale.
    parameters = {
      type = "object",
      properties = {
        verb = { type = "string", description = "What to do. `help` lists them all." },
        who = {
          type = "string",
          description = "Which instance: `iota-mu`, `review/iota-mu` or `axon/review/iota-mu`.",
        },
        message = { type = "string", description = "What to say, for the verbs that say something." },
        about = { type = "string", description = "The id of the message being answered." },
        sort = { type = "string", description = "For `send` only: what kind of message it is." },
      },
      required = { "verb" },
    },

    transport = {
      kind = "command",
      command = "atom",
      -- An argument the model left out still arrives, as an empty string. atom drops those
      -- rather than looking for an instance named "", so a `list` needs no `who`.
      args = {
        "tool",
        "--verb", "{verb}",
        "--who", "{who}",
        "--message", "{message}",
        "--about", "{about}",
        "--sort", "{sort}",
      },
      timeout = 30,
    },
  })
end
