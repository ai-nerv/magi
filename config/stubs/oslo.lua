-- oslo's client library: what another program requires to talk to a running shell.
--
--   local src  = io.popen("oslo lua-api"):read("a")
--   local oslo = load(src)(my_transport)
--   local sh   = oslo.connect()
--   print(sh.env.get("PATH"))
--
-- Plain Lua on purpose. It runs unchanged in oslo's own VM, in ziglua, in PUC Lua and in whatever
-- the next sibling embeds, so it is *copied* between tools rather than ported — and a fix to the
-- framing reaches every one of them.
--
-- The only thing it cannot do itself is open a socket. That arrives as the chunk's argument:
--
--   load(src)(transport)     transport.connect(path, timeout_ms) -> handle
--                            handle:send(bytes) / handle:recv(n) / handle:close()
--
-- Inside oslo that is `oslo.stream`, and it is found automatically when nothing is passed.
--
-- **The surface is small on purpose.** It is not a mirror of `oslo.*`; it is the handful of things
-- another program has a real reason to ask a shell, and every one of them answers a question the
-- asker cannot answer for itself. Nothing here runs a command — see `oslo.live` on the server side
-- for why that is a decision and not an omission.

local transport = ...

-- Where the socket primitive comes from when the caller did not say. Inside oslo the whole library
-- is already there; elsewhere a host that named its own `__stream` is honoured too.
if not transport then
  transport = (_G.oslo and _G.oslo.stream) or _G.__stream
end

local M = { _NAME = "oslo", _VERSION = 1 }

-- Every global this family answers to. The file is copied between siblings, so a lookup that knew
-- only its own name would fail on exactly the hosts it is meant to run in.
local FAMILY = { "oslo", "hexe" }

-- ---------------------------------------------------------------- JSON, in Lua

-- Carried rather than required. The library runs inside somebody else's VM, so it cannot reach
-- oslo's own `oslo.json` — and a client that depended on the host having a JSON module would be a
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
  if depth > 24 then error("oslo: value nested too deeply to send", 0) end

  local kind = type(v)
  if v == nil then return "null" end
  if kind == "boolean" then return tostring(v) end
  if kind == "string" then return quote(v) end
  if kind == "number" then
    if v ~= v or v == math.huge or v == -math.huge then
      error("oslo: " .. tostring(v) .. " cannot be sent", 0)
    end
    return (math.type and math.type(v) == "integer") and string.format("%d", v) or tostring(v)
  end
  if kind ~= "table" then error("oslo: cannot send a " .. kind, 0) end

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

  local function fail(why) error("oslo: bad reply at " .. at .. ": " .. why, 0) end
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
          -- byte-for-byte cases go through \u00xx, which is what oslo escapes control bytes as.
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
--
-- **A stream delivers what it likes.** One `recv` answering fewer bytes than asked for is ordinary,
-- not an error, and a client that treated it as the whole message would desynchronise on the first
-- reply large enough to be split.
local function exactly(handle, n)
  local parts, have = {}, 0
  while have < n do
    local chunk, why = handle:recv(n - have)
    if not chunk then return nil, why end
    if #chunk == 0 then return nil, "the shell closed the connection" end
    parts[#parts + 1] = chunk
    have = have + #chunk
  end
  return table.concat(parts)
end

-- ---------------------------------------------------------------- the session

local Session = {}
Session.__index = Session

--- Send one call and wait for its answer.
function Session:call(name, ...)
  if not self.handle then return nil, "this connection is closed" end
  local request = encode({ call = name, args = { ... } })
  local sent, why = self.handle:send(frame(request))
  if not sent then return nil, why end

  local head, gone = exactly(self.handle, 4)
  if not head then return nil, gone end
  local body, cut = exactly(self.handle, be32(head))
  if not body then return nil, cut end

  local reply = decode(body)
  if not reply.ok then return nil, reply.error or "the shell refused the call" end
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

--- `verbs` -- every name this shell will answer, asked of the shell rather than assumed -- is
--- reached through SURFACE below, like every other call: `sh.verbs()`.
---
--- There is deliberately no `Session:verbs` method. `attach` sets every SURFACE name as a field on
--- the instance, and a field shadows a method, so defining one would not give callers a second way
--- to call it -- it would only advertise a form that does not work. `sh:verbs()` passes the session
--- as the first argument and dies in the encoder with "cannot send a function", which says nothing
--- about the cause. Every verb is called with a dot.

-- The exposed surface, spelled out.
--
-- Written as a table rather than discovered from `verbs()` above so that reading this file tells
-- you what a peer can do to your shell. A surface you have to run something to learn is one nobody
-- audits.
local SURFACE = {
  "cwd", "session", "verbs",
  "env.get", "env.all", "env.set",
  "macros.get",
  "notify",
  -- Asks; does not do. `sh.cd(dir)` answers true once the shell has *accepted* the move, which is
  -- before it has made it: a shell running a foreground command gets there when that ends. Read
  -- `sh.cwd()` back if you need to know it happened.
  "cd",
}

local function attach(session)
  for _, path in ipairs(SURFACE) do
    local head, tail = path:match("^(.-)%.(.+)$")
    if head then
      session[head] = session[head] or {}
      session[head][tail] = function(...) return session:call(path, ...) end
    else
      session[path] = function(...) return session:call(path, ...) end
    end
  end
  return session
end

-- ---------------------------------------------------------------- connecting

--- Where a shell's socket is, given what little the caller said.
---
--- `$OSLO_SOCK` first, because a process the shell started inherits it and means *that* shell. A
--- session id names one exactly. With neither, the newest socket wins, which is right for the
--- common case of one shell and honest about being a guess when there are several.
--- Answers a *list* of candidates, newest first, because a socket file is not a running shell: one
--- left behind by a shell that was killed looks exactly like a live one until something connects.
--- Trying them in turn is the only staleness check that cannot be raced.
local function find(where)
  if type(where) == "table" and where.path then return { { path = where.path } } end
  local named = type(where) == "string" and where or nil

  local env = os.getenv("OSLO_SOCK")
  if not named and env and env ~= "" then return { { path = env } } end

  local runtime = os.getenv("XDG_RUNTIME_DIR")
  local dir = (runtime and runtime ~= "")
    and (runtime .. "/onix/oslo")
    or ("/tmp/onix-" .. (os.getenv("UID") or "0") .. "/oslo")
  if named then return { { path = dir .. "/" .. named .. ".sock" } } end

  -- No id and no env var: the newest socket in the directory. Plain Lua cannot list one, so this
  -- asks the host two ways and gives up rather than guessing.
  --
  -- **`io.popen` is the fallback, not the first choice.** It shells out, and oslo's own VM refuses
  -- it outright — a client running *inside* a shell would raise here rather than answering. So a
  -- host that can list a directory itself is asked first, and the shell-out is wrapped where it
  -- might not exist at all.
  -- Whichever sibling we are running inside. This file is copied between tools, so it looks for
  -- any of the family's globals rather than only its own: inside hexe `_G.oslo` does not exist,
  -- and checking only for that sent discovery straight to the `io.popen` below — which a sandboxed
  -- host refuses, failing as "no oslo socket found" when one was running all along.
  local host
  for _, name in ipairs(FAMILY) do
    local candidate = _G[name]
    if candidate and candidate.fs and candidate.fs.ls then host = candidate; break end
  end
  if host then
    local found = {}
    for _, entry in ipairs(host.fs.ls(dir) or {}) do
      if entry.name:sub(-5) == ".sock" then
        found[#found + 1] = { path = dir .. "/" .. entry.name, when = entry.mtime or 0 }
      end
    end
    table.sort(found, function(a, b) return a.when > b.when end)
    return found
  end

  local ok, found = pcall(function()
    local ls = io.popen("ls -t '" .. dir .. "'/*.sock 2>/dev/null")
    if not ls then return nil end
    local out = {}
    for line in ls:lines() do out[#out + 1] = { path = line } end
    ls:close()
    return out
  end)
  return ok and found or nil
end

--- Open a connection to a running oslo.
---
--- `where` is nothing (find it), a session id, or `{ path = "…", timeout_ms = 5000 }`.
function M.connect(where)
  if not transport then
    return nil, "no transport: pass one to the chunk, as load(src)(oslo.stream)"
  end
  local candidates = find(where)
  if not candidates or #candidates == 0 then
    return nil, "no oslo socket found — is one serving? see `oslo.live.serve()`"
  end

  local timeout = type(where) == "table" and where.timeout_ms or nil
  local last
  for _, candidate in ipairs(candidates) do
    local handle, why = transport.connect(candidate.path, timeout)
    if handle then
      return attach(setmetatable({ handle = handle, path = candidate.path }, Session))
    end
    last = why
  end
  return nil, last or "nothing was listening"
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
  for _, name in ipairs(FAMILY) do
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
  for _, name in ipairs(FAMILY) do
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
function M.where(id)
  local candidates = find(id)
  return candidates and candidates[1] and candidates[1].path
end

--- Every candidate, newest first, for a caller that wants to choose.
function M.sockets(id)
  local out = {}
  for _, candidate in ipairs(find(id) or {}) do out[#out + 1] = candidate.path end
  return out
end

return M
