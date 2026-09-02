-- magi's client library: what another program requires to talk to a running agent.
--
--   local magi = load(src)(my_transport)
--   local agent = magi.connect()
--   for _, s in ipairs(agent.sessions()) do print(s.id, s.cwd, s.status) end
--
-- Three ways to get `src`, and a sandboxed host can only use the first two: the `client` verb on
-- an already-open connection, a sibling's own client (the framing and reply shape are shared), or
-- `io.popen("magi lua-api"):read("a")` where the host permits `io`.
--
-- Plain Lua on purpose. It runs unchanged in magi's own VM, in hexe's, in oslo's, in PUC Lua and in whatever
-- the next sibling embeds, so it is *copied* between tools rather than ported — and a fix to the
-- framing reaches every one of them. It is a sibling of oslo's `client.lua` and deliberately the
-- same shape; read that one and this one holds no surprises.
--
-- The only thing it cannot do itself is open a socket. That arrives as the chunk's argument:
--
--   load(src)(transport)     transport.connect(path, timeout_ms) -> handle
--                            handle:send(bytes) / handle:recv(n) / handle:close()
--
-- Inside magi that is `magi.stream`, and it is found automatically when nothing is passed.
--
-- **The surface is small on purpose.** It is not a mirror of the local API; it is the handful of
-- things another program has a real reason to ask a mux, and every one answers a question the
-- asker cannot answer for itself — a pane list scraped from CLI output is a guess, this is not.
--
-- What a session will actually let you do is decided at the far end, per call, by the access the
-- caller was granted (see docs/access.md). This file describes the vocabulary; it does not grant
-- anything.

local transport = ...

-- Where the socket primitive comes from when the caller did not say. Inside magi the whole library
-- is already there; elsewhere a host that named its own `__stream` is honoured too.
if not transport then
  transport = (_G.magi and _G.magi.stream) or _G.__stream
end

local M = { _NAME = "magi", _VERSION = 1 }

-- Every global this family answers to. The file is copied between siblings, so a lookup that knew
-- only its own name would send discovery down the `io.popen` path on exactly the hosts that refuse
-- it -- which reads as "nothing is running".
local HOSTS = { "magi", "hexe", "oslo" }

-- ---------------------------------------------------------------- JSON, in Lua

-- Carried rather than required. The library runs inside somebody else's VM, so it cannot reach
-- magi's own JSON — and a client that depended on the host having a JSON module would be a client
-- most hosts could not load.

local ESCAPES = {
  ['"'] = '\\"', ['\\'] = '\\\\', ['\b'] = '\\b',
  ['\f'] = '\\f', ['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t',
}

local function quote(s)
  return '"' .. s:gsub('[%c"\\]', function(c)
    return ESCAPES[c] or string.format('\\u%04x', c:byte())
  end) .. '"'
end

local encode

local function is_list(t)
  local n = 0
  for _ in pairs(t) do n = n + 1 end
  return n == #t
end

function encode(v, depth)
  depth = (depth or 0) + 1
  if depth > 24 then error("magi: value nested too deeply to send", 0) end

  local kind = type(v)
  if v == nil then return "null" end
  if kind == "boolean" then return tostring(v) end
  if kind == "string" then return quote(v) end
  if kind == "number" then
    if v ~= v or v == math.huge or v == -math.huge then
      error("magi: " .. tostring(v) .. " cannot be sent", 0)
    end
    return (math.type and math.type(v) == "integer") and string.format("%d", v) or tostring(v)
  end
  if kind ~= "table" then error("magi: cannot send a " .. kind, 0) end

  local out = {}
  if is_list(v) then
    for i = 1, #v do out[#out + 1] = encode(v[i], depth) end
    return "[" .. table.concat(out, ",") .. "]"
  end
  for key, value in pairs(v) do
    if type(key) ~= "string" then error("magi: only string keys can be sent", 0) end
    out[#out + 1] = quote(key) .. ":" .. encode(value, depth)
  end
  return "{" .. table.concat(out, ",") .. "}"
end

local function decode(s)
  local at = 1

  local function fail(why)
    error("magi: bad reply at " .. at .. ": " .. why, 0)
  end

  local function skip()
    while true do
      local c = s:sub(at, at)
      if c == " " or c == "\t" or c == "\n" or c == "\r" then at = at + 1 else return end
    end
  end

  local function literal(word, value)
    if s:sub(at, at + #word - 1) == word then
      at = at + #word
      return value, true
    end
    return nil, false
  end

  local value

  local function str()
    at = at + 1
    local out = {}
    while true do
      local c = s:sub(at, at)
      if c == "" then fail("unterminated string") end
      if c == '"' then at = at + 1; return table.concat(out) end
      if c == "\\" then
        local e = s:sub(at + 1, at + 1)
        at = at + 2
        if e == "n" then out[#out + 1] = "\n"
        elseif e == "t" then out[#out + 1] = "\t"
        elseif e == "r" then out[#out + 1] = "\r"
        elseif e == "b" then out[#out + 1] = "\b"
        elseif e == "f" then out[#out + 1] = "\f"
        elseif e == "u" then
          local hex = s:sub(at, at + 3)
          at = at + 4
          local code = tonumber(hex, 16) or 0
          -- Enough for what this surface carries; a full UTF-16 pair decoder is not.
          out[#out + 1] = (code < 128) and string.char(code) or "?"
        else out[#out + 1] = e end
      else
        out[#out + 1] = c
        at = at + 1
      end
    end
  end

  function value()
    skip()
    local c = s:sub(at, at)
    if c == '"' then return str() end
    if c == "{" then
      at = at + 1
      local out = {}
      skip()
      if s:sub(at, at) == "}" then at = at + 1; return out end
      while true do
        skip()
        if s:sub(at, at) ~= '"' then fail("wanted a key") end
        local key = str()
        skip()
        if s:sub(at, at) ~= ":" then fail("wanted ':'") end
        at = at + 1
        out[key] = value()
        skip()
        local sep = s:sub(at, at)
        at = at + 1
        if sep == "}" then return out end
        if sep ~= "," then fail("wanted ',' or '}'") end
      end
    end
    if c == "[" then
      at = at + 1
      local out = {}
      skip()
      if s:sub(at, at) == "]" then at = at + 1; return out end
      while true do
        out[#out + 1] = value()
        skip()
        local sep = s:sub(at, at)
        at = at + 1
        if sep == "]" then return out end
        if sep ~= "," then fail("wanted ',' or ']'") end
      end
    end
    local got, found = literal("true", true)
    if found then return got end
    got, found = literal("false", false)
    if found then return got end
    got, found = literal("null", nil)
    if found then return got end

    local number = s:match("^-?%d+%.?%d*[eE]?[-+]?%d*", at)
    if number and #number > 0 then
      at = at + #number
      return tonumber(number) or fail("bad number")
    end
    fail("unexpected " .. (c == "" and "end of reply" or ("'" .. c .. "'")))
  end

  return value()
end

-- ---------------------------------------------------------------- the frame

-- Four bytes of big-endian length, then the body. Written by hand rather than with `string.pack`,
-- which Lua 5.1 and LuaJIT do not have and one of the siblings might be.
local function frame(body)
  local n = #body
  return string.char(
    math.floor(n / 16777216) % 256,
    math.floor(n / 65536) % 256,
    math.floor(n / 256) % 256,
    n % 256
  ) .. body
end

local function be32(s)
  return s:byte(1) * 16777216 + s:byte(2) * 65536 + s:byte(3) * 256 + s:byte(4)
end

--- Read exactly `n` bytes, however many reads that takes.
---
--- **A stream delivers what it likes.** One `recv` answering fewer bytes than asked for is
--- ordinary, not an error, and a client that treated it as the whole message would desynchronise
--- on the first reply large enough to be split.
local function exactly(handle, n)
  local parts, have = {}, 0
  while have < n do
    local chunk, why = handle:recv(n - have)
    if not chunk then return nil, why end
    if #chunk == 0 then return nil, "the session closed the connection" end
    parts[#parts + 1] = chunk
    have = have + #chunk
  end
  return table.concat(parts)
end

-- ---------------------------------------------------------------- the session

local Session = {}
Session.__index = Session

--- Send one call and wait for its answer.
---
--- magi serves one request per connection, so this reconnects each time. That is the server's
--- contract, not an accident: a control socket that held connections open would need a slot per
--- idle caller, and it has eight.
function Session:call(name, ...)
  local args = { ... }
  local body = { call = name }
  if #args > 0 then body.args = args end

  local handle, why = transport.connect(self.path, self.timeout_ms)
  if not handle then return nil, why end

  local sent, gone = handle:send(frame(encode(body)))
  if not sent then handle:close(); return nil, gone end

  local head, cut = exactly(handle, 4)
  if not head then handle:close(); return nil, cut end
  local reply_body, trimmed = exactly(handle, be32(head))
  handle:close()
  if not reply_body then return nil, trimmed end

  local reply = decode(reply_body)
  if not reply.ok then return nil, reply.error or "the session refused the call" end
  -- `result` is a list of return values and `n` says how many, so one Lua call
  -- answers with what the remote one did. The same shape oslo's server uses:
  -- a client that unpacked a bare-value reply lost every record, string
  -- and number it was handed.
  local values = reply.result or {}
  return table.unpack(values, 1, reply.n or #values)
end

function Session:close()
  return true
end

-- The exposed surface, spelled out.
--
-- Written as a table rather than discovered at runtime so that reading this file tells you what a
-- peer can ask of your session. A surface you have to run something to learn is one nobody audits.
-- What another program has a real reason to ask a coding agent, and every one answers a question
-- the asker cannot answer for itself: which sessions exist and where they are rooted, what each is
-- doing right now, what it has said. A shell scraping `ps` is guessing; this is not.
--
-- **Read-only, deliberately.** There is no `prompt`, `submit` or `run` here. A coding agent
-- executes shell commands and edits files on the model's say-so, so a socket verb that hands it a
-- prompt is remote code execution wearing a friendlier name — and the fact that a person would
-- have typed the same thing does not make it the same thing when a peer types it. Anything that
-- causes work stays behind the local API, where the person who owns the session is the one asking.
--
-- `verbs` ships from the first version. One sibling having it and another not is how a family
-- stops being one, and it cannot be retrofitted quietly.
local SURFACE = {
  "sessions", "session", "verbs",
  "status", "transcript", "cwd",
  "models",
}

local function attach(session)
  for _, verb in ipairs(SURFACE) do
    session[verb] = function(...) return session:call(verb, ...) end
  end
  return session
end

-- ---------------------------------------------------------------- connecting

--- Where a session's socket is, given what little the caller said.
---
--- `$MAGI_API_SOCKET` first, because a program magi started inherits it and means *that* session —
--- a plugin never has to guess. A session name picks one exactly. With neither, the newest socket
--- wins, which is right for the common case of one session and honest about being a guess when
--- there are several.
---
--- Answers a *list* of candidates, newest first, because a socket file is not a running session:
--- one left behind by a frontend that was killed looks exactly like a live one until something
--- connects. Trying them in turn is the only staleness check that cannot be raced.
--- Every `api@*.sock` under `dir`, newest first. Empty when nothing can list it.
---
--- Plain Lua cannot list a directory, so this asks the host two ways and gives up rather than
--- guessing.
---
--- **`io.popen` is the fallback, not the first choice.** It shells out, and a sandboxed host may
--- refuse it outright, so a host that can list a directory itself is asked first and the shell-out
--- is wrapped where it might not exist at all.
---
--- Whichever sibling we are running inside: this file is meant to be copied between them, so it
--- looks for any of the family's globals rather than only its own. Inside oslo `_G.magi` does not
--- exist, and checking only for that sent discovery straight to the `io.popen` oslo refuses —
--- which looked exactly like "no session is running".
local function list_candidates(dir)
  local host
  for _, name in ipairs(HOSTS) do
    local candidate = _G[name]
    if candidate and candidate.fs and candidate.fs.ls then host = candidate; break end
  end

  if host then
    local found = {}
    for _, entry in ipairs(host.fs.ls(dir) or {}) do
      if entry.name:sub(1, 4) == "api@" and entry.name:sub(-5) == ".sock" then
        found[#found + 1] = { path = dir .. "/" .. entry.name, when = entry.mtime or 0 }
      end
    end
    table.sort(found, function(a, b) return a.when > b.when end)
    return found
  end

  local ok, found = pcall(function()
    local ls = io.popen("ls -t '" .. dir .. "'/api@*.sock 2>/dev/null")
    if not ls then return nil end
    local out = {}
    for line in ls:lines() do out[#out + 1] = { path = line } end
    ls:close()
    return out
  end)
  return (ok and found) or {}
end

--- The directory magi binds its control sockets in.
---
--- Mirrors magi's own socket directory: the default session binds straight under `<runtime>/magi`,
--- and only a NAMED instance gets a subdirectory. Appending "default" unconditionally looked
--- reasonable and found nothing at all.
local function socket_dir()
  -- The host's own answer first: a sandboxed file has no `os.getenv`, and this is a path it can be
  -- handed rather than a reason to grant it every environment variable.
  for _, name in ipairs(HOSTS) do
    local h = _G[name]
    if h and h.fs and h.fs.dir then
      local d = h.fs.dir()
      if d and d ~= "" then return d end
    end
  end
  local runtime = os.getenv("XDG_RUNTIME_DIR")
  local base = (runtime and runtime ~= "")
    and (runtime .. "/magi")
    or ("/tmp/magi-" .. (os.getenv("UID") or "0"))
  local instance = os.getenv("HEXE_INSTANCE")
  return (instance and instance ~= "") and (base .. "/" .. instance) or base
end

--- Where a session's socket is, given what little the caller said.
---
--- `$MAGI_API_SOCKET` first, because a program magi started inherits it and means *that* session —
--- a plugin never has to guess. A name picks one. With neither, the newest socket wins, which is
--- right for the common case of one session and honest about being a guess when there are several.
---
--- Answers a *list*, newest first, because a socket file is not a running session: one left behind
--- by a frontend that was killed looks exactly like a live one until something connects. Trying
--- them in turn is the only staleness check that cannot be raced.
---
--- A name is tried as the file first, then against what each session CALLS itself. Those differ
--- more often than you would think: the file is named when the socket binds, and a session renamed
--- or reattached afterwards keeps the old file — so the name a caller read from `session().name`
--- may name no file at all.
local function find(where)
  if type(where) == "table" and where.path then return { { path = where.path } } end
  local named = type(where) == "string" and where or nil

  local env = os.getenv("MAGI_API_SOCKET")
  if not named and env and env ~= "" then return { { path = env } } end

  local dir = socket_dir()
  if not named then return list_candidates(dir) end

  local direct = dir .. "/api@" .. named .. ".sock"
  local out = { { path = direct } }
  for _, candidate in ipairs(list_candidates(dir)) do
    if candidate.path ~= direct then
      out[#out + 1] = { path = candidate.path, must_be_named = named }
    end
  end
  return out
end

--- Open a connection to a running magi.
---
--- `where` is nothing (find it), a session name, or `{ path = "…", timeout_ms = 5000 }`.
function M.connect(where)
  if not transport then
    return nil, "no transport: pass one to the chunk, as load(src)(magi.stream)"
  end
  local candidates = find(where)
  if not candidates or #candidates == 0 then
    return nil, "no magi socket found - is a daemon running?"
  end

  local timeout = type(where) == "table" and where.timeout_ms or nil

  -- Refuse our own session, when the host told us which that is.
  --
  -- A call to yourself from inside your own event loop cannot be answered: the
  -- frontend is busy running the caller. It looks like a hang and ends in a
  -- socket timeout, which says nothing about the cause. Inside magi, `ctx.*`
  -- already reaches everything this library does, without a socket.
  local self_socket = _G.magi and _G.magi.__self_socket
  local last
  for _, candidate in ipairs(candidates) do
    if self_socket and candidate.path == self_socket then
      return nil, "that is this session; use ctx.* in here rather than connecting to yourself"
    end
    local handle, why = transport.connect(candidate.path, timeout)
    if handle then
      handle:close()
      local session = attach(setmetatable({ path = candidate.path, timeout_ms = timeout }, Session))
      -- A candidate reached by scanning has to prove it is the one asked for.
      -- Asking it its own name is the only way: the file name is a snapshot
      -- from when it bound, and this is what it answers to now.
      if candidate.must_be_named then
        local live = session.session()
        if not (live and live.name == candidate.must_be_named) then
          last = "no session answers to '" .. candidate.must_be_named .. "'"
          goto continue
        end
      end
      return session
    end
    last = why
    ::continue::
  end
  return nil, last or "nothing was listening"
end

--- Open a connection to the pane this code is running in.
---
--- A different door, not a filtered view of `connect()`: the pane's socket answers only for that
--- pane, and refuses by name anything about the session. A client cannot widen it, because the
--- narrowing is the listener's, decided when magi bound it.
---
--- Grants nothing to the caller it did not already have -- it IS the process in that pane -- but it
--- lets it ask magi precisely instead of parsing its own terminal, and it means a program handed
--- this path can be trusted with it.
---
--- `$MAGI_SESSION_API_SOCKET` is exported into every pane's shell. Nothing is listening on it while no
--- frontend is attached, which is the truth about a live API rather than a fault to hide.
function M.connect_pane(opts)
  local path = os.getenv("MAGI_SESSION_API_SOCKET")
  if not path or path == "" then
    return nil, "not inside an magi session ($MAGI_SESSION_API_SOCKET is unset)"
  end
  local session, why = M.connect({ path = path, timeout_ms = type(opts) == "table" and opts.timeout_ms or nil })
  if not session then
    return nil, (why or "nothing was listening") .. " — is a frontend attached to this session?"
  end
  return session
end

-- ---------------------------------------------------------------- one question

--- A SYNCHRONOUS command runner, from whichever sibling we are loaded into.
---
--- `run(cmd, opts) -> { ok, code, stdout, stderr }`, answered before it returns. Deliberately not
--- the same thing as a statusbar's `exec`: that kind is asynchronous and cached -- it answers
--- `pending` first and the real result later -- which is right for painting a prompt and useless
--- for a request that has to be answered now.
---
--- A host that must not block is entitled to lend nothing here. A terminal multiplexer is the
--- clearest case: a spawn inside its frontend loop suspends every pane in the session for as long
--- as the child takes, so it lends no `run` and `fetch` over a spawn simply is not available in it.
--- Sockets still are.
local function host_run()
  for _, name in ipairs(HOSTS) do
    local h = _G[name]
    if h and type(h.run) == "function" then return h.run end
  end
  return nil
end

--- A spawnable tool's descriptor, if it left one beside the sockets.
---
--- Discovery stays implicit, but what it finds is an ABSOLUTE path the tool wrote about itself --
--- never a name resolved through `$PATH`. Executing whatever answers to a name, on the failure path
--- where nothing was listening, is not the same risk as opening a socket something chose to create.
--- A tool whose state IS its process simply never writes one of these, so "do not spawn me" is a
--- fact this can check rather than a rule someone has to remember.
--- Where a named tool keeps its runtime files. The same shape as `socket_dir`, for a sibling
--- rather than for us -- a descriptor has to be findable by whoever is asking, not only by its
--- author, so it lives under the tool's own name and not under ours.
local function tool_dir(tool)
  local runtime = os.getenv("XDG_RUNTIME_DIR")
  if runtime and runtime ~= "" then return runtime .. "/" .. tool end
  return "/tmp/" .. tool .. "-" .. (os.getenv("UID") or "0")
end

local function descriptor(tool)
  if type(tool) ~= "string" or tool == "" or tool:find("[^%w._-]") then return nil end
  local host, path = nil, tool_dir(tool) .. "/" .. tool .. ".tool"
  for _, name in ipairs(HOSTS) do
    local h = _G[name]
    if h and h.fs and h.fs.ls then host = h; break end
  end
  if not host or not host.fs.read then return nil end
  local body = host.fs.read(path)
  if not body then return nil end
  local ok, d = pcall(decode, body)
  if not ok or type(d) ~= "table" or type(d.exec) ~= "string" then return nil end
  return d
end

--- Ask one question and take the answer. No handle, nothing held, nothing to close.
---
--- `connect()` is a channel with a lifetime; this is not, and the difference is not an
--- implementation detail worth hiding behind one name. The verb says what the CALLER wanted, so a
--- tool that later grows a daemon breaks no call site.
---
--- Two ways to be answered, tried in that order:
---   * a socket, when one is listening -- ask without holding it;
---   * the tool's own `api` subcommand, for a tool that has no daemon and never will.
---
--- `where` is a session name, `{ tool = "name" }`, or nothing.
function M.fetch(where, verb, ...)
  if type(verb) ~= "string" then return nil, "fetch needs a verb: fetch(where, 'name', ...)" end

  -- Only OUR sockets answer our verbs. Asking for a different tool must not resolve to one of
  -- ours and call a name it has never heard of -- an error at the far end reads as the tool being
  -- broken rather than as the wrong peer being asked.
  local asked = type(where) == "table" and where.tool or nil
  local session = (asked == nil or asked == M._NAME) and M.connect(where) or nil
  if session then
    local out = table.pack(session:call(verb, ...))
    session:close()
    return table.unpack(out, 1, out.n)
  end

  local tool = type(where) == "table" and where.tool or (type(where) == "string" and where or M._NAME)
  local d = descriptor(tool)
  if not d then
    return nil, "nothing is listening for '" .. tostring(tool) .. "', and it left no .tool descriptor"
  end
  local exec = host_run()
  if not exec then
    return nil, "'" .. tool .. "' has no socket and this host lends no synchronous runner, so it "
      .. "cannot be spawned from in here — ask from a host that can block, or give it a daemon"
  end

  -- The request is argv and the reply is stdout: one question needs no framing, and asking for one
  -- costs a primitive that takes stdin, which not every host lends.
  local argv = { d.exec }
  for _, a in ipairs(d.args or {}) do argv[#argv + 1] = a end
  argv[#argv + 1] = verb
  for i = 1, select("#", ...) do argv[#argv + 1] = encode((select(i, ...))) end
  local quoted = {}
  for i, a in ipairs(argv) do quoted[i] = "'" .. tostring(a):gsub("'", "'\\''") .. "'" end

  local r = exec(table.concat(quoted, " "), { timeout_ms = (type(where) == "table" and where.timeout_ms) or 5000 })
  if not r or not r.ok then
    return nil, "'" .. tool .. "' failed: " .. tostring(r and (r.stderr ~= "" and r.stderr or r.code) or "no result")
  end
  local decoded, why = pcall(decode, r.stdout or "")
  if not decoded then return nil, "'" .. tool .. "' did not answer in the family's shape: " .. tostring(why) end
  local reply = why
  if not reply.ok then return nil, reply.error or "the tool refused the call" end
  return table.unpack(reply.result or {}, 1, reply.n or #(reply.result or {}))
end

--- The socket path that would be tried first, without connecting. For a diagnostic.
function M.where(name)
  local candidates = find(name)
  return candidates and candidates[1] and candidates[1].path
end

--- Every candidate, newest first, for a caller that wants to choose.
function M.sockets(name)
  local out = {}
  for _, candidate in ipairs(find(name) or {}) do out[#out + 1] = candidate.path end
  return out
end

return M
