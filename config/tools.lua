-- The tools magi ships.
--
-- magi ships no tool that does anything to the machine. Reading, writing, editing, searching and
-- running a command are all the tools program's (casper's), spawned per call: `cat` reads, `shell`
-- writes and runs, `patch` diffs, `ls`/`find`/`grep` search. magi's own `read`/`write`/`edit`
-- builtins were removed once casper filled the role — see `magi-tools` and `ROLES.md`.
--
-- What is left here is what is not a tool in casper's sense. The memory role's tools are this
-- session's own memory; `agent` reaches the other magi through melchior. Both are about *this
-- harness's* relationships rather than about doing something to the machine, which is the line
-- casper is on the other side of.
--
-- Each reaches its sibling through **either door**, chosen by `magi.memory.door` / `magi.agent.door`:
--   library -- the sibling's own client library in this VM, over its socket (`sibling` below).
--   command -- the sibling's command line, one exec per call (the `command` transport).
-- Both are wired for both tools. The defaults differ only because one door costs something on each:
-- `agent` defaults to `command`, because melchior takes who is calling from the kernel and a root
-- session's own process cannot present that over the socket (a child can); `memory` defaults to
-- `library`, because balthasar's library carries the injection/outcome loop the command line drops.

-- Load a sibling's client library and run it against the socket primitive -- the library door.
local function sibling(name)
  local source = magi.clients and magi.clients[name]
  if not source then return nil, name .. "'s client library is not installed" end
  local chunk, why = load(source, name .. ".lua")
  if not chunk then return nil, why end
  return chunk(magi.stream)
end

do -- the memory role
  -- Which program fills the `memory` role -- see ROLES.md. `magi.roles` is what the configuration
  -- said, and `balthasar` is who has always filled it when nothing says otherwise. The name is
  -- read here and used everywhere below, so pointing `magi.memory` at another program moves this
  -- whole block to it without a word changing.
  local PROGRAM = (magi.roles and magi.roles.memory) or "balthasar"

  -- Which door reaches it, from `$MAGI_MEMORY_DOOR` (a config global cannot: `magi tools` and the
  -- session rebuild a fresh VM that re-runs only the declared tools, not `init.lua`, and `os.getenv`
  -- is what reaches that VM). `library` (default) keeps balthasar's injection/outcome loop and the
  -- `history` read; `command` runs balthasar's own CLI per call and is the plainer of the two.
  local DOOR = os.getenv("MAGI_MEMORY_DOOR") or "library"

  -- The memory layer's client, when the library door is asked for and the library is in hand: a
  -- library here means that program ran on this machine a moment ago. Read at load, because a tool
  -- has to exist before the model is told what it may call. A memory layer that lends nothing is the
  -- ordinary case, not an error: the session runs without memory tools, as every one did before.
  local memory = select(1, sibling(PROGRAM))

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

  if DOOR == "command" then
    -- The core verbs through balthasar's command line, one exec per call. `{arg}` is filled from
    -- the call, its own token dropped when absent. `history` and the injection/outcome loop below
    -- are the library door's, and are not offered here.
    for name, shape in pairs(MEMORY) do
      local argv = { name }
      for _, key in ipairs(shape.args) do
        argv[#argv + 1] = "{" .. key .. "}"
      end
      magi.tool(name, {
        description = shape.about,
        parameters = shape.parameters,
        transport = { kind = "command", command = PROGRAM, args = argv },
      })
    end
  elseif memory then
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
  if DOOR ~= "command" and memory and magi.session then
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
  -- Talking to the other magi sessions in this project, through melchior -- a separate program that
  -- owns naming, the sockets sessions reach each other on, and the walls between them.
  --
  -- Reached through either door (`magi.agent.door`), the same `tool` vocabulary either way:
  --   command (default) -- `melchior tool --verb=…`, one exec per call. Works in every session,
  --                        because the fresh child carries this session's identity in its environment.
  --   library           -- melchior's client library over the socket, like the memory block. Works
  --                        in a *child* session; a root session's own process cannot present its
  --                        identity to melchior's socket, so `command` is the honest default.
  local COORD = (magi.roles and (magi.roles.coordination or magi.roles.model)) or "melchior"
  -- From `$MAGI_AGENT_DOOR`, for the reason the memory block gives. `command` (default) works in
  -- every session; `library` works in a child but not a root (melchior cannot verify a root's own
  -- process over the socket).
  local DOOR = os.getenv("MAGI_AGENT_DOOR") or "command"
  local mel = DOOR == "library" and select(1, sibling(COORD)) or nil

  -- Command always declares (it fails per call if melchior is absent, which is the honest outcome);
  -- library declares only when the client is in hand.
  if DOOR ~= "library" or mel then
    local spec = {
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
    }

    if DOOR == "library" then
      -- The client library, over the socket melchior's `serve` bound, calling the `tool` verb --
      -- `nil` is this session's own melchior. A refusal comes back as `nil, why`, handled here.
      spec.transport = { kind = "lua" }
      spec.run = function(args)
        args = args or {}
        if not args.verb or args.verb == "" then
          return { content = 'agent needs a `verb`; `verb: "help"` lists them', is_error = true }
        end
        local map = { verb = args.verb }
        for _, key in ipairs({ "who", "message", "about", "role", "sort" }) do
          if args[key] ~= nil and args[key] ~= "" then map[key] = args[key] end
        end
        local said, why = mel.fetch(nil, "tool", map)
        if not said then return { content = tostring(why), is_error = true } end
        return { content = said }
      end
    else
      -- One exec of `melchior tool --verb=…`. `--name={value}`, one token, never `"--name",
      -- "{value}"` as two: an argument the model left out is dropped whole only when the flag and
      -- the placeholder are the same token, or the bare flag swallows the next argument's value.
      spec.transport = {
        kind = "command",
        command = COORD,
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
      }
    end

    magi.tool("agent", spec)
  end
end

-- There is no `process` or `mcp` transport, and no in-magi shell: running a tool is casper's, and
-- an external tool server belongs behind the `tools` role, not inside magi. The two transports a
-- declaration here may use are `lua` (a function in this VM, for reaching a sibling like the memory
-- block above) and `command` (one exec of another program, as `agent` reaches melchior).
