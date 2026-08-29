-- hexe's client library: what another program requires to talk to a running session.
-- Plain Lua, so siblings copy it rather than port it. The transport arrives as the chunk's
-- argument; inside axum that is `axum.stream`, found automatically when nothing is passed.
--
--   local hexe = load(src)(my_transport)
--   local mux  = hexe.connect()
--   for _, pane in ipairs(mux.panes()) do print(pane.name, pane.cwd) end

local transport = ...

-- Where the socket primitive comes from when the caller did not say. Inside hexe the whole library
-- is already there; elsewhere a host that named its own `__stream` is honoured too.
if not transport then
  transport = (_G.hexe and _G.hexe.stream) or _G.__stream
end

local M = { _NAME = "hexe", _VERSION = 1 }

-- Every global this family answers to. The file is copied between siblings, so a lookup that knew
-- only its own name would send discovery down the `io.popen` path on exactly the hosts that refuse
-- it -- which reads as "nothing is running".
local HOSTS = { "hexe", "oslo", "axum" }

-- ---------------------------------------------------------------- JSON, in Lua

-- Carried rather than required. The library runs inside somebody else's VM, so it cannot reach
-- hexe's own JSON — and a client that depended on the host having a JSON module would be a client
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
  if depth > 24 then error("hexe: value nested too deeply to send", 0) end

  local kind = type(v)
  if v == nil then return "null" end
  if kind == "boolean" then return tostring(v) end
  if kind == "string" then return quote(v) end
  if kind == "number" then
    if v ~= v or v == math.huge or v == -math.huge then
      error("hexe: " .. tostring(v) .. " cannot be sent", 0)
    end
    return (math.type and math.type(v) == "integer") and string.format("%d", v) or tostring(v)
  end
  if kind ~= "table" then error("hexe: cannot send a " .. kind, 0) end

  local out = {}
  if is_list(v) then
    for i = 1, #v do out[#out + 1] = encode(v[i], depth) end
    return "[" .. table.concat(out, ",") .. "]"
  end
  for key, value in pairs(v) do
    if type(key) ~= "string" then error("hexe: only string keys can be sent", 0) end
    out[#out + 1] = quote(key) .. ":" .. encode(value, depth)
  end
  return "{" .. table.concat(out, ",") .. "}"
end

local function decode(s)
  local at = 1

  local function fail(why)
    error("hexe: bad reply at " .. at .. ": " .. why, 0)
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
  -- a client that unpacked hexe's old single value lost every record, string
  local values = reply.result or {}
  return table.unpack(values, 1, reply.n or #values)
end

function Session:close()
  return true
end

-- The exposed surface, spelled out.
local SURFACE = {
  "panes", "pane", "tabs", "session", "ui", "count", "verbs",
  "screen_text", "line",
  "send", "keys",
  "notify", "popup", "capture",
  "focus",
}

local function attach(session)
  for _, verb in ipairs(SURFACE) do
    session[verb] = function(...) return session:call(verb, ...) end
  end
  return session
end

-- ---------------------------------------------------------------- connecting

--- Where a session's socket is, given what little the caller said.
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

--- The directory hexe binds its control sockets in.
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
    and (runtime .. "/hexe")
    or ("/tmp/hexe-" .. (os.getenv("UID") or "0"))
  local instance = os.getenv("HEXE_INSTANCE")
  return (instance and instance ~= "") and (base .. "/" .. instance) or base
end

--- Where a session's socket is, given what little the caller said.
local function find(where)
  if type(where) == "table" and where.path then return { { path = where.path } } end
  local named = type(where) == "string" and where or nil

  local env = os.getenv("HEXE_API_SOCKET")
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

--- Open a connection to a running hexe.
function M.connect(where)
  if not transport then
    return nil, "no transport: pass one to the chunk, as load(src)(hexe.stream)"
  end
  local candidates = find(where)
  if not candidates or #candidates == 0 then
    return nil, "no hexe socket found — is a session attached?"
  end

  local timeout = type(where) == "table" and where.timeout_ms or nil

  -- Refuse our own session, when the host told us which that is.
  local self_socket = _G.hexe and _G.hexe.__self_socket
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
function M.connect_pane(opts)
  local path = os.getenv("HEXE_PANE_API_SOCKET")
  if not path or path == "" then
    return nil, "not inside a hexe pane ($HEXE_PANE_API_SOCKET is unset)"
  end
  local session, why = M.connect({ path = path, timeout_ms = type(opts) == "table" and opts.timeout_ms or nil })
  if not session then
    return nil, (why or "nothing was listening") .. " — is a frontend attached to this session?"
  end
  return session
end

-- ---------------------------------------------------------------- one question

--- A SYNCHRONOUS command runner, from whichever sibling we are loaded into.
local function host_run()
  for _, name in ipairs(HOSTS) do
    local h = _G[name]
    if h and type(h.run) == "function" then return h.run end
  end
  return nil
end

--- A spawnable tool's descriptor, if it left one beside the sockets.
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
