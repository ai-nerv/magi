-- The tools magi ships.
--
-- magi ships no tool that does anything to the machine. Every one of those — `read`, `write`,
-- `edit`, `shell`, `ls`, `find`, `grep`, `hexe`, `oslo` — is the tools program's (casper's), run in
-- a process spawned per call. `read`, `write` and `edit` were magi's own for a while and are casper's
-- now, like the rest: moved, not copied, so there is never a second declaration of one name.
--
-- What is left here is what is not a tool in casper's sense. The memory role's tools are this
-- session's own memory; `agent` reaches the other magi through melchior. Both are about *this
-- harness's* relationships rather than about doing something to the machine, which is the line
-- casper is on the other side of.

do -- the memory role
  -- Which program fills the `memory` role -- see ROLES.md. `magi.roles` is what the configuration
  -- said, and `balthasar` is who has always filled it when nothing says otherwise. The name is
  -- read here and used everywhere below, so pointing `magi.memory` at another program moves this
  -- whole block to it without a word changing.
  local PROGRAM = (magi.roles and magi.roles.memory) or "balthasar"

  -- The memory layer, if it is installed. Whether it is, is answered by the client library in
  -- hand: magi asks each sibling for one by running it, so a library here means that program
  -- ran on this machine a moment ago. Nothing else at config time knows as much -- a
  -- socket file outlives its process, and a live one may still be too busy to answer.
  local function client()
    local source = magi.clients and magi.clients[PROGRAM]
    if not source then return nil, PROGRAM .. "'s client library is not installed" end
    local chunk, why = load(source, PROGRAM .. ".lua")
    if not chunk then return nil, why end
    return chunk(magi.stream)
  end

  -- Read at load, because a tool has to exist before the model is told what it may call. A memory
  -- layer that lends nothing is the ordinary case, not an error: nothing is registered and the
  -- session runs without memory tools, which is what every session did before there was one.
  local memory = select(1, client())

  -- The last context the memory layer handed over. A recall that comes back with an injection id
  -- is it saying "these went into your model's context, tell me what you did with them" -- and this
  -- is the only place that id is held, because nothing else in magi needs to know it exists.
  local injection = nil

  -- Which memory layer to ask.
  --
  -- **Named, never guessed.** A memory layer's client falls back to the newest socket in the
  -- runtime directory when nobody says which -- right for the common case of one session, and a
  -- coin flip the moment somebody opens a second window in the same project. magi starts its own
  -- and knows exactly where it put it, so it says.
  --
  -- Absent outside a session (`magi tools` builds a VM to list what is declared) and absent when
  -- there is no memory layer at all, and then this is `{ tool = PROGRAM }` exactly as before.
  local OURS = { tool = PROGRAM, path = magi.balthasar_at }

  -- Which verbs the model gets, and their whole declaration. The rest are the harness's --
  -- `observe`, `replay` and the transcript plumbing magi drives in Rust, not through here.
  --
  -- **Local, because a declaration cannot wait on a round trip.** These were registered from
  -- whatever `verbs()` answered, which made the model's memory depend on balthasar replying at
  -- the instant this VM is built. It does not always: a balthasar that serves one caller at a
  -- time is busy with the connection magi records the transcript over, and one still opening its
  -- store answers nothing either. The ask came back empty, nothing registered, and a session
  -- recording into a live balthasar offered the model no way to ask it anything.
  --
  -- Nothing was lost by writing them here. The schemas were always local -- balthasar publishes
  -- no argument shapes -- and what `verbs()` supplied was its own one-line signature, written for
  -- `balthasar verbs` to print and not for a model to read. A verb balthasar drops is now refused
  -- per call in balthasar's own words rather than silently absent, which is what the `agent`
  -- block below does about melchior for the same reason.
  local MEMORY = {
    recall = {
      args = { "query" },
      about = "Search the memory layer for what earlier sessions in this project learned: " ..
        "decisions, conventions, and things that cost somebody time to find out.",
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
      about = "Keep something for later sessions: a decision, a convention, or a fact about " ..
        "this project that was not obvious.",
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
      about = "Archive a memory that is wrong or no longer true, by the id `recall` gave it.",
      parameters = {
        type = "object",
        properties = { id = { type = "string", description = "Which memory." } },
        required = { "id" },
      },
    },
    why = {
      args = { "id" },
      about = "Say what a memory rests on: how confident it is, and which sessions asserted it.",
      parameters = {
        type = "object",
        properties = { id = { type = "string", description = "Which memory." } },
        required = { "id" },
      },
    },
  }

  if memory then
    for name, shape in pairs(MEMORY) do
      magi.tool(name, {
        description = shape.about,
        parameters = shape.parameters,
        transport = { kind = "lua" },
        run = function(args)
          args = args or {}
          local positional = {}
          for i, key in ipairs(shape.args) do positional[i] = args[key] end
          local answer, why =
            memory.fetch(OURS, name, table.unpack(positional, 1, #shape.args))
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
  -- The counterpart to masking, and the reason masking is safe.
  --
  -- balthasar shrinks a full window by replacing a big tool result with a stub -- "`shell` output
  -- elided (~1200 tokens)" -- because tool output is most of a coding session's window and a
  -- summary is the expensive lossy last resort. That is reversible only if something can fetch
  -- the text back, and until now nothing could: the stub said "run it again", which for a
  -- half-hour test run is a poor answer when the output is sitting in balthasar's scrollback.
  --
  -- `scroll` is the read for everything except restoring a session, and it is bounded: balthasar
  -- says what it left out and where to continue from, so a model asking for a long history gets
  -- a page rather than the window it was trying to save.
  --
  -- Declared here rather than in MEMORY above because it takes this session's id first, which the
  -- model has no business supplying and no way to know. `magi.session` is absent in a VM nobody
  -- named a session for -- `magi tools` has one -- and then this tool is simply not offered.
  if memory and magi.session then
    magi.tool("history", {
      description =
        "Read earlier parts of this conversation back out of the memory layer. " ..
        "Use it when a tool result has been elided to save context and you need what it said, " ..
        "or to find something said far enough back that it is no longer in view.",
      parameters = {
        type = "object",
        properties = {
          want = {
            type = "string",
            enum = { "tail", "around", "matching" },
            description =
              "tail: the most recent turns. around: what surrounds one turn. " ..
              "matching: turns mentioning your terms.",
          },
          cursor = { type = "integer", description = "Which turn, for `around`." },
          terms = {
            type = "array",
            items = { type = "string" },
            description = "Words to look for, for `matching`.",
          },
          tokens = {
            type = "integer",
            description = "How much to read back at most. Keep it small: this spends the window you are trying to save.",
          },
        },
      },
      transport = { kind = "lua" },
      run = function(args)
        args = args or {}
        local want = args.want or "tail"
        if want == "around" and not args.cursor then
          return { content = "`around` needs a cursor -- the turn to read either side of.", is_error = true }
        end
        if want == "matching" and not args.terms then
          return { content = "`matching` needs terms to look for.", is_error = true }
        end
        -- Capped here as well as by balthasar. Its own default is generous for a plugin reading a
        -- history; this is a model spending its own context to get one back.
        local tokens = math.min(tonumber(args.tokens) or 2000, 8000)
        local answer, why = memory.fetch(OURS, "scroll", magi.session, {
          want = want,
          cursor = args.cursor,
          terms = args.terms,
          tokens = tokens,
        })
        if not answer then return { content = tostring(why), is_error = true } end
        return { content = magi.json.encode(answer) }
      end,
    })
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
      if not memory or not injection then return end
      -- Only tool events. magi tells watchers about turns, permissions, compaction and the
      -- session too, and every one of those arrives here with no `tool` field at all.
      if event.kind ~= "tool.finished" then return end
      if event.tool == "recall" or event.tool == "remember" then return end

      -- What the tool was actually asked to do, as one string. balthasar hashes it and keeps the
      -- digest; the arguments themselves never leave this VM.
      local args = event.arguments or {}
      local action = args.command or args.path or args.query or ""

      local used = memory.fetch(OURS, "used", injection, {
        tool = event.tool,
        action = action,
      })
      if not used or not used.action then return end

      memory.fetch(OURS, "outcome", used.action, {
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
  Talk to the other magi instances running in this project.

  `verb: "help"` is the list of every verb and what each takes, generated by the melchior that is
  actually installed. Call it before you decide you cannot do something. What follows is only what
  that list cannot tell you: what a verb does to the instance on the far end, and which capabilities
  change how you should plan rather than what you can call.

  ANSWERING SOMEBODY. A message from another instance appears in this conversation as a block
  headed `<RELATION::id>`. Replying in your own text does NOT reach them -- they cannot see this
  conversation. To answer, call this tool:

    1. `verb: "inbox"` -- lists what has been sent to you, each with an id.
    2. `verb: "reply", who: <their id>, about: <that message's id>, message: <your answer>`.

  `about` is required by `reply` and must be an id from `inbox`; without it the call is refused.
  If you have nothing to quote, use `send` or `ask` instead rather than guessing an id.

  WAKING SOMEBODY. What you pick decides whether they start a turn:

    `ask`       you need an answer; it starts a turn for them, and their reply starts one for you
    `reply`     answers a question you were asked; starts a turn for whoever asked
    `send`      a note. It does NOT wake them -- they read it next time they answer something
    `attention` you need them now; this is the one that reaches them mid-turn
    `trouble`   something is wrong and you cannot go on
    `handoff`   this piece of work is theirs now

  So: use `ask` when you want a response, `send` only when you genuinely want no reply.

  BEFORE YOU START A PIECE OF WORK. Claims are how two instances avoid doing the same thing twice.
  `claims` says what everyone has taken; `claim`, with `about` naming the work, records it as
  yours, and `release` gives it back under the same name. They are advisory -- nothing stops you
  working on something claimed -- so reading `claims` is part of deciding, not a formality.

  WHO IS THERE, AND WHAT FOR. `list` says which instances you can actually reach and how each
  relates to you; use it rather than assuming a name. `crew` is the whole roster of this run,
  including instances `list` cannot reach. What an instance is *for* is a role, and roles are what
  work is routed by: `role` says what this session is for, `assign` says what one this session
  started is for. Both take `role` as one word and `message` as the sentence a coordinator reads.

  An instance this session started can also be ended, and any it did not start cannot: the verb is
  in `help`, and the refusal is melchior's, not this description's.

  Instances are named `id`, `role/id` or `project/role/id`; a bare id means one in this project.]],

    -- Six arguments and no list of verbs, on purpose. melchior's vocabulary grows and this file
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
        about = {
          type = "string",
          description =
            "What is being named: the message being answered, for `reply`; the piece of work " ..
            "being taken or let go, for `claim` and `release`.",
        },
        role = {
          type = "string",
          description =
            "For `role` and `assign`: one word to call the role, like `reviewer`. `message` " ..
            "then says what it does, in a sentence.",
        },
        sort = {
          type = "string",
          description =
            "For `send` only: what kind of message it is. Every kind has a verb of its own, " ..
            "and the verb is the one that is checked -- prefer it. A sort melchior does not " ..
            "know is read as a plain note, which wakes nobody.",
        },
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
        "--role={role}",
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
