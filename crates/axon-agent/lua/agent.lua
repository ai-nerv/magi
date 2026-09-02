-- The agent layer's client library: what another program requires to talk to a running axon.
-- Plain Lua, so siblings copy it rather than port it. The transport arrives as the chunk's
-- argument; inside axon that is `axon.stream`, found automatically when nothing is passed.
--
--   local agent = load(src)(my_transport)
--   local them  = agent.connect("beta-nu")
--   print(them.status())
--
-- What is on the other end is one *session* — one axon, one conversation, one name. There is no
-- server for the layer as a whole: every session binds its own socket and answers for itself, so
-- "connect to the agent layer" is always "connect to somebody".
--
--   $XDG_RUNTIME_DIR/axon/<project>/<id>          the socket a session listens on
--   $XDG_RUNTIME_DIR/axon/<project>/<id>.parent   who started it, when somebody did
--
-- A name is `project/role/id`, and the role is what a session says it is *for* — it never
-- decides which socket is meant. The id does. So `review/iota-mu` and `scratch/iota-mu` are the
-- same session described two ways, and `agent.connect` reaches it by the last part either way.

local transport = ...

-- Where the socket primitive comes from when the caller did not say. Inside axon the whole
-- library is already there; elsewhere a host that named its own `__stream` is honoured too.
if not transport then
  transport = (_G.axon and _G.axon.stream) or _G.__stream
end

local M = { _NAME = "agent", _VERSION = 1 }

-- Every global this family answers to. The file is copied between siblings, so a lookup that knew
-- only its own name would fail on exactly the hosts it is meant to run in.
local FAMILY = { "axon", "aeon", "oslo", "hexe" }

-- ---------------------------------------------------------------- JSON, in Lua

-- Carried rather than required. The library runs inside somebody else's VM, so it cannot reach
-- the host's own JSON module — and a client that depended on there being one would be a
-- client most hosts could not load.

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

-- A Lua table is a map or a list and JSON is not, so the shape has to be decided. An empty table
-- encodes as an array: every argument list is one, and an empty *record* is not something the
-- surface below ever sends.
local function is_list(t)
  local n = 0
  for _ in pairs(t) do n = n + 1 end
  return n == #t
end

function encode(v, depth)
  depth = (depth or 0) + 1
  if depth > 24 then error("agent: value nested too deeply to send", 0) end

  local kind = type(v)
  if v == nil then return "null" end
  if kind == "boolean" then return tostring(v) end
  if kind == "string" then return quote(v) end
  if kind == "number" then
    if v ~= v or v == math.huge or v == -math.huge then
      error("agent: " .. tostring(v) .. " cannot be sent", 0)
    end
    return (math.type and math.type(v) == "integer") and string.format("%d", v) or tostring(v)
  end
  if kind ~= "table" then error("agent: cannot send a " .. kind, 0) end

  local out = {}
  if is_list(v) then
    for i = 1, #v do out[#out + 1] = encode(v[i], depth) end
    return "[" .. table.concat(out, ",") .. "]"
  end
  for key, value in pairs(v) do
    out[#out + 1] = quote(tostring(key)) .. ":" .. encode(value, depth)
  end
  return "{" .. table.concat(out, ",") .. "}"
end

local function decode(s)
  local at = 1

  local function fail(why) error("agent: bad reply at " .. at .. ": " .. why, 0) end
  local function skip() at = s:find("[^ \t\r\n]", at) or #s + 1 end

  local function literal(word, value)
    if s:sub(at, at + #word - 1) ~= word then return nil, false end
    at = at + #word
    return value, true
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
          local code = tonumber(hex, 16) or fail("bad \\u")
          -- A lone surrogate or anything past the BMP is not something this surface sends; the
          -- byte-for-byte cases go through \u00xx, which is what the family escapes control bytes as.
          out[#out + 1] = (code < 256) and string.char(code)
            or (utf8 and utf8.char(code) or "?")
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

  local out = value()
  return out
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

-- Read exactly `n` bytes, however many reads that takes.
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
--- Every call says who is making it. That is not courtesy: a session answers `verbs` to anybody
--- and refuses everything else without a `from`, because every other verb is about that session
--- and a stranger has no standing to ask. The name is taken at face value — one user, one
--- directory, nothing to check it against — and what it buys is a *relation*, worked out at the
--- far end from the project directory rather than from anything in this frame.
function Session:call(name, ...)
  if not self.handle then return nil, "this connection is closed" end
  local request = encode({ call = name, args = { ... }, from = self.from, token = self.token })
  local sent, why = self.handle:send(frame(request))
  if not sent then return nil, why end

  local head, gone = exactly(self.handle, 4)
  if not head then return nil, gone end
  local body, cut = exactly(self.handle, be32(head))
  if not body then return nil, cut end

  local reply = decode(body)
  -- A refusal is a reply, not a dropped connection: "only the session that started one may stop
  -- it" says what to fix where "connection reset" does not.
  if not reply.ok then return nil, reply.error or "the session refused the call" end
  -- `result` is a list of return values, so one Lua call answers with what the remote one did.
  return table.unpack(reply.result or {}, 1, reply.n or #(reply.result or {}))
end

function Session:close()
  if self.handle then
    self.handle:close()
    self.handle = nil
  end
  return true
end

-- The verbs a session answers. Small on purpose and not a mirror of anything: most of what a
-- session knows is meaningless to a peer, and the parts that are dangerous — anything that runs
-- a command — are not here and are not coming.
--
-- Ask `verbs()` rather than trusting this list. It is here so `them.status()` reads as a call
-- and not as a string, and a session that answers something newer will say so.
local SURFACE = {
  "verbs", "client",
  "identity", "kin", "status", "inbox",
  "tell",
  -- The one act the far end cannot decline, so the one that has to prove itself: it carries the
  -- secret the session was started with, and a session nobody started holds none.
  "stop",
}

local function attach(session)
  for _, verb in ipairs(SURFACE) do
    session[verb] = function(...) return session:call(verb, ...) end
  end
  return session
end

-- ---------------------------------------------------------------- who is where

--- The runtime directory sessions live under.
local function runtime()
  local dir = os.getenv("XDG_RUNTIME_DIR")
  if dir and dir ~= "" then return dir .. "/axon" end
  return "/tmp/axon-" .. (os.getenv("UID") or "0")
end

--- Who this process belongs to, if it belongs to a session at all.
---
--- Set by the session that started the process, and inherited from there by everything it
--- starts. It is the one thing a separate process cannot work out for itself: names are made
--- when a session starts, and nothing on disk says which of several a given process came from.
function M.me()
  local project, id = os.getenv("AXON_PROJECT"), os.getenv("AXON_ID")
  if not project or project == "" or not id or id == "" then return nil end
  local role = os.getenv("AXON_ROLE")
  if not role or role == "" then role = "main" end
  return { project = project, role = role, id = id, full = project .. "/" .. role .. "/" .. id }
end

--- List a directory, however this host can.
---
--- Plain Lua cannot, so this asks the host two ways and gives up rather than guessing. A wrong
--- guess here is the worst kind: it reads as "nobody is running" and sends whoever asked away.
local function entries(dir)
  for _, name in ipairs(FAMILY) do
    local host = _G[name]
    if host and host.fs and host.fs.ls then
      local out = {}
      for _, entry in ipairs(host.fs.ls(dir) or {}) do out[#out + 1] = entry.name end
      return out
    end
  end
  local ok, found = pcall(function()
    local ls = io.popen("ls -1 '" .. dir .. "' 2>/dev/null")
    if not ls then return nil end
    local out = {}
    for line in ls:lines() do out[#out + 1] = line end
    ls:close()
    return out
  end)
  return (ok and found) or {}
end

--- Every session listening in `project`, by id.
---
--- Read from the directory rather than from a registry somebody has to keep up to date: a
--- process that died did not get to remove itself from a list. A socket file outlives its
--- process, so a name here is not a promise that anything is behind it — [`M.answers`] is the
--- cheapest way to find out, and `list` on a session is the considered one.
---
--- An id is two Greek words and a dash, so the `.parent`, `.host`, `.pid` and `.log` files that
--- sit beside the sockets are told apart by the dot they contain.
function M.instances(project)
  project = project or (M.me() or {}).project
  if not project then return nil, "no project: pass one, or set AXON_PROJECT" end
  local out = {}
  for _, name in ipairs(entries(runtime() .. "/" .. project)) do
    if not name:find("%.") and name ~= "" then out[#out + 1] = name end
  end
  table.sort(out)
  return out
end

--- Where a session's socket is.
---
--- The id is the part that places it. A role in the path would be a second key for the same
--- door, and a session that changed what it was for would move.
local function socket_of(project, id)
  return runtime() .. "/" .. project .. "/" .. id
end

--- Read a name the way a session would: `id`, `role/id`, or `project/role/id`.
---
--- From the right, because the id is the part that is always there and the rest fills in from
--- whoever is asking. `a/b/c/d` is not a deeper name, it is a typo.
local function read(name, mine)
  local parts = {}
  for part in tostring(name):gsub("^%$", ""):gmatch("[^/]+") do parts[#parts + 1] = part end
  mine = mine or M.me() or {}
  if #parts == 1 then return mine.project, parts[1] end
  if #parts == 2 then return mine.project, parts[2] end
  if #parts == 3 then return parts[1], parts[3] end
  return nil, nil
end

-- ---------------------------------------------------------------- connecting

--- Open a connection to a running session.
---
--- `where` is a name — `"iota-mu"`, `"review/iota-mu"`, `"axon/review/iota-mu"` — or a table
--- with a `path`. Nothing means this process's own session, which is how a peer asks itself
--- what has arrived.
---
--- The connection is held. Closing after one call is the obvious way to write a client and it
--- dies on its *second* call with a broken pipe, so this hands back something with a `close`.
function M.connect(where)
  if not transport then
    return nil, "no transport: pass one to the chunk, as load(src)(axon.stream)"
  end
  local mine = M.me()
  local path, named
  if type(where) == "table" and where.path then
    path = where.path
  else
    local project, id
    if where == nil then
      if not mine then return nil, "no session: name one, or set AXON_PROJECT and AXON_ID" end
      project, id = mine.project, mine.id
    else
      project, id = read(where, mine)
    end
    if not project or not id then
      return nil, "`" .. tostring(where) .. "` is not a name a session can have"
    end
    named = project .. "/" .. id
    path = socket_of(project, id)
  end

  local timeout = type(where) == "table" and where.timeout_ms or nil
  local handle, why = transport.connect(path, timeout)
  if not handle then
    return nil, "nothing is listening as " .. (named or path) .. (why and (" (" .. why .. ")") or "")
  end
  return attach(setmetatable({
    handle = handle,
    path = path,
    -- Put on every call. Without it a session answers `verbs` and refuses everything else,
    -- which presents as "that instance does not work" rather than as a client that never
    -- introduced itself.
    from = mine and mine.full or nil,
    token = os.getenv("AXON_TOKEN"),
  }, Session))
end

--- Whether anything is actually listening as `where`.
---
--- A socket file outlives the process that made it, so the directory says who *was* there. This
--- is the cheapest question that tells a running session from a crash's leftovers.
function M.answers(where)
  local session = M.connect(where)
  if not session then return false end
  session:close()
  return true
end

-- ---------------------------------------------------------------- one question

--- Ask one thing and keep nothing.
---
--- The counterpart to `connect`: that is a channel you hold and close, this is a question. The
--- verb says what the *caller* wanted, so a call site written today still reads correctly when
--- the layer grows something to hold a connection open for.
function M.fetch(where, verb, ...)
  local session, why = M.connect(where)
  if not session then return nil, why end
  local answers = table.pack(session:call(verb, ...))
  session:close()
  return table.unpack(answers, 1, answers.n)
end

--- This file's own source, as the session it is talking to has it.
---
--- `agent lua-api` prints it, which is enough for a host that can shell out and useless to one
--- that cannot: a sandboxed VM with no `io.popen` has no way to run it. So a session hands the
--- library out over the wire too, and a sibling that speaks the framing can fetch the right
--- vocabulary using the wrong one, in code, with nothing written to disk.
function M.client(where)
  return M.fetch(where, "client")
end

return M
