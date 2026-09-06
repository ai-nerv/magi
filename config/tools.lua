-- The tools magi ships.
--
-- `shell` runs in a peer process because running commands is the thing most worth isolating;
-- the other two ask a sibling that is already running, over the socket the family shares, so
-- they are functions in this VM. A tool of your own goes in either camp.

-- `shell`, `hexe`, `oslo`, `ls`, `find` and `grep` were declared here and are casper's now.
-- Moved, not copied: two declarations of one name is the state where somebody edits the one that
-- lost, and registration is keyed, so the loser sits here doing nothing and still looking
-- maintained.
--
-- What is left is what is not a tool in casper's sense. `balthasar` is this session's own memory,
-- registered from whatever verbs that instance answers; `agent` reaches the other magi through
-- melchior. Both are about *this harness's* relationships rather than about doing something to
-- the machine, which is the line casper is on the other side of.
--
-- `read`, `write` and `edit` are not here at all — they are compiled in, as the floor a session
-- can never be without. See `magi-tools`.

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

-- An MCP server, if you have one you want here.
--
-- Uncomment and point it at whatever you run. `kind = "mcp"` is the one declaration that
-- registers *more than one* tool: an MCP server publishes a list, and the names the model sees
-- are the server's own -- so `name` here names the *server*, and never becomes a tool.
--
-- Nothing else about it is special, which is the whole design. Each tool it publishes registers
-- beside a builtin, a Lua tool and a casper tool; is checked against the schema the server
-- published; asks the same person for the same permission; and is capped and masked on the way
-- back like any other. The turn loop does not know MCP exists.
--
-- `sha256` pins the server to the bytes you wrote this against. An MCP server is somebody else's
-- code, running as you, with your tools -- and `command` is a name that resolves to whatever is
-- on `$PATH` today. Unpinned is the ordinary case and starts fine; `magi doctor` prints what each
-- server actually hashed to, which is where the value below comes from.
--
-- do
--   magi.tool("filesystem", {
--     description = "files, from the reference MCP server",
--     parameters = { type = "object" },
--     transport = {
--       kind = "mcp",
--       command = "npx",
--       args = { "-y", "@modelcontextprotocol/server-filesystem", "/home/you/work" },
--       -- sha256 = "…",  -- from `magi doctor`
--     },
--   })
-- end
