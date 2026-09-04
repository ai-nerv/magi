-- The tools magi ships.
--
-- `shell` runs in a peer process because running commands is the thing most worth isolating;
-- the other two ask a sibling that is already running, over the socket the family shares, so
-- they are functions in this VM. A tool of your own goes in either camp.

do -- shell
  magi.tool("shell", {
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
      -- magi is a multi-call binary, so its own shell peer is the same executable under
      -- another name. `magi.self` is the path of the binary that is running: naming it "magi"
      -- and hoping PATH agrees finds whichever copy the shell sees, and an older one fails as
      command = magi.self,
      args = { "ext", "shell" },
    },
  })
end

-- `ls`, `find` and `grep` were declared here and are casper's now. Not copied -- moved: two
-- declarations of one name is the state where somebody edits the wrong one, and registration is
-- keyed, so the copy that lost would have sat here doing nothing and still looking maintained.
--
-- `shell` stays. casper's `bash` runs one command per exec, and this one holds a process: `cd
-- build` and then `make` has to work, and a stateless shell would quietly stop being able to do
-- it. It moves when casper's can keep a working directory between calls, not before.

do -- hexe
  -- The client arrives as source in `magi.clients`: a config cannot open files.
  local function client()
    local source = magi.clients and magi.clients.hexe
    if not source then return nil, "hexe's client library is not installed; run `make configs`" end
    local chunk, why = load(source, "hexe.lua")
    if not chunk then return nil, why end
    return chunk(magi.stream)
  end


  -- Discovery is the client's, for the same reason it is oslo's: `magi` is in the client's host list
  -- now, so it can list the socket directory from in here rather than shelling out to a VM that
  -- refuses. What was hand-rolled here happened to match hexe's layout and so happened to work --


  magi.tool("hexe", {
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
      return { content = magi.json.encode(answer) }
    end,
  })
end

do -- oslo
  -- The client arrives as source in `magi.clients`: a config cannot open files.
  local function client()
    local source = magi.clients and magi.clients.oslo
    if not source then return nil, "oslo's client library is not installed; run `make configs`" end
    local chunk, why = load(source, "oslo.lua")
    if not chunk then return nil, why end
    return chunk(magi.stream)
  end


  -- Discovery is the client's. It was hand-rolled here because the client could not list a directory
  -- from inside magi -- its list of hosts to ask named `oslo` and `hexe` and not the one it was
  -- running in -- and the workaround guessed a layout: `$XDG_RUNTIME_DIR/oslo/api@*.sock`, which is


  magi.tool("oslo", {
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
      return { content = magi.json.encode(answer) }
    end,
  })
end

do -- balthasar
  -- The memory layer, if it is installed and running. balthasar publishes its own tool descriptors --
  -- `remember`, `recall`, `forget` -- so the vocabulary is written once, in balthasar, rather than
  -- copied here to drift.
  local function client()
    local source = magi.clients and magi.clients.balthasar
    if not source then return nil, "balthasar's client library is not installed" end
    local chunk, why = load(source, "balthasar.lua")
    if not chunk then return nil, why end
    return chunk(magi.stream)
  end

  -- Asked at load, because a tool has to exist before the model is told what it may call. balthasar
  -- being absent is the ordinary case, not an error: nothing is registered and the session runs
  -- without memory, which is what every session did before balthasar existed.
  local balthasar = select(1, client())

  -- The last context balthasar handed over. A recall that comes back with an injection id is balthasar
  -- saying "these went into your model's context, tell me what you did with them" -- and this
  -- is the only place that id is held, because nothing else in magi needs to know it exists.
  local injection = nil

  -- Which verbs the model gets. balthasar serves nineteen; the rest are the harness's --
  -- `observe`, `replay` and the transcript plumbing magi drives in Rust, not through here.
  -- The prose comes from balthasar's own `verbs()`; the schemas do not, because balthasar
  -- publishes no argument shapes and a tool cannot be declared without them.
  local MEMORY = {
    recall = {
      args = { "query" },
      parameters = {
        type = "object",
        properties = {
          query = { type = "string", description = "What to look for." },
        },
        required = { "query" },
      },
    },
    remember = {
      args = { "text" },
      parameters = {
        type = "object",
        properties = {
          text = { type = "string", description = "The thing worth keeping." },
        },
        required = { "text" },
      },
    },
    forget = {
      args = { "id" },
      parameters = {
        type = "object",
        properties = { id = { type = "string", description = "Which memory." } },
        required = { "id" },
      },
    },
    why = {
      args = { "id" },
      parameters = {
        type = "object",
        properties = { id = { type = "string", description = "Which memory." } },
        required = { "id" },
      },
    },
  }

  local asked, offered = pcall(function()
    return balthasar and balthasar.fetch({ tool = "balthasar" }, "verbs")
  end)
  if asked and type(offered) == "table" then
    for _, v in ipairs(offered) do
      local shape = MEMORY[v.name]
      if shape then
        magi.tool(v.name, {
          description = v.about or v.name,
          parameters = shape.parameters,
          transport = { kind = "lua" },
          run = function(args)
            args = args or {}
            local positional = {}
            for i, name in ipairs(shape.args) do positional[i] = args[name] end
            local answer, why =
              balthasar.fetch({ tool = "balthasar" }, v.name, table.unpack(positional, 1, #shape.args))
            if not answer then return { content = tostring(why), is_error = true } end
            -- Kept, and stripped from what the model sees. The id is bookkeeping between magi
            -- and balthasar; putting it in the context would spend tokens on a handle the model
            -- can do nothing with, and invite it to make one up.
            if type(answer) == "table" and answer.injection then
              injection = answer.injection
              answer = answer.memories or answer
            end
            return { content = magi.json.encode(answer) }
          end,
        })
      end
    end
  end

  -- Close the loop. Every tool that finishes after balthasar handed something over is reported back:
  -- what ran, and whether it worked. balthasar decides for itself whether the action followed any of
  -- the memories it gave -- magi does not guess, because a harness claiming a match it did not
  -- verify is asserting an analysis rather than reporting an action.
  --
  -- Nothing here is required. With balthasar absent, or its ledger off, `injection` stays nil and
  -- this never fires; the session runs exactly as it did before.
  magi.watch("balthasar-outcome", {
    run = function(event)
      if not balthasar or not injection then return end
      if event.tool == "recall" or event.tool == "remember" then return end

      -- What the tool was actually asked to do, as one string. balthasar hashes it and keeps the
      -- digest; the arguments themselves never leave this VM.
      local args = event.arguments or {}
      local action = args.command or args.path or args.query or ""

      local used = balthasar.fetch({ tool = "balthasar" }, "used", injection, {
        tool = event.tool,
        action = action,
      })
      if not used or not used.action then return end

      balthasar.fetch({ tool = "balthasar" }, "outcome", used.action, {
        kind = event.is_error and "failed" or "succeeded",
      })
    end,
  })
end

do -- agent
  -- Talking to the other magi sessions in this project, through melchior -- a separate program that owns
  -- naming, the sockets sessions reach each other on, and the walls between them.
  --
  -- `melchior` rather than `magi ext agent`, and a `command` rather than a `process`: this ran as
  -- magi's own peer until the layer left, and neither half of that is a rename. A command
  -- transport is one exec per call with the arguments in argv, which is the whole protocol melchior
  -- offers -- deliberately, so a harness that can run a program can use it without copying
  -- anybody's message types.
  --
  -- Delete this block if melchior is not installed. The tool then fails per call rather than at
  -- load, which is the honest outcome: a session with no melchior has no siblings to talk to.
  magi.tool("agent", {
    description = [[
  Talk to the other magi instances running in this project: ask what they are doing, send them
  work, answer their questions, and stop the ones this session started.

  ANSWERING SOMEBODY. A message from another instance appears in this conversation as a block
  headed `<RELATION::id>`. Replying in your own text does NOT reach them -- they cannot see this
  conversation. To answer, call this tool:

    1. `verb: "inbox"` -- lists what has been sent to you, each with an id.
    2. `verb: "reply", who: <their id>, about: <that message's id>, message: <your answer>`.

  `about` is required by `reply` and must be an id from `inbox`; without it the call is refused.
  If you have nothing to quote, use `send` or `ask` instead rather than guessing an id.

  CHOOSING A VERB. What you pick decides whether they wake up:

    `ask`       you need an answer; it starts a turn for them, and their reply starts one for you
    `reply`     answers a question you were asked; starts a turn for whoever asked
    `send`      a note. It does NOT wake them -- they read it next time they answer something
    `attention` you need them now; this is the one that reaches them mid-turn
    `trouble`   something is wrong and you cannot go on
    `handoff`   this piece of work is theirs now

  So: use `ask` when you want a response, `send` only when you genuinely want no reply.

  Instances are named `id`, `role/id` or `project/role/id`; a bare id means one in this project.
  `list` says who is actually there -- use it rather than assuming a name. `verb: "help"` lists
  every verb and what each takes, from the melchior that is actually installed.]],

    -- Five arguments and no list of verbs, on purpose. melchior's vocabulary grows and this file
    -- would not hear about it; `help` is the copy that cannot go stale.
    parameters = {
      type = "object",
      properties = {
        verb = { type = "string", description = "What to do. `help` lists them all." },
        who = {
          type = "string",
          description = "Which instance: `iota-mu`, `review/iota-mu` or `magi/review/iota-mu`.",
        },
        message = { type = "string", description = "What to say, for the verbs that say something." },
        about = { type = "string", description = "The id of the message being answered." },
        sort = { type = "string", description = "For `send` only: what kind of message it is." },
      },
      required = { "verb" },
    },

    transport = {
      kind = "command",
      command = "melchior",
      -- `--name={value}`, one token, and never `"--name", "{value}"` as two.
      --
      -- An argument the model left out is dropped *whole*, flag and all -- but only when the
      -- flag and the placeholder are the same token. Written as two, the placeholder vanishes
      -- and the bare flag stays, so `reply` with no `about` sent `--about --sort` and melchior read
      -- the next flag as the value: `about` came out as the string "--sort". The verb was then
      -- refused for want of a real one, the model fell back to `send`, and the answer arrived as
      -- a note -- which wakes nobody. One exchange, then silence, from a missing `=`.
      args = {
        "tool",
        "--verb={verb}",
        "--who={who}",
        "--message={message}",
        "--about={about}",
        "--sort={sort}",
      },
      timeout = 30,
    },
  })
end
