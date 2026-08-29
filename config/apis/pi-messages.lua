-- Pi's own protocol, for a pi-compatible endpoint.

local M = {}

function M.endpoint(base_url, _model)
  return base_url .. "/messages"
end

function M.headers(key)
  if not key then return {} end
  return { authorization = "Bearer " .. key }
end

local function block(c)
  if c.type == "text" then
    if c.text == "" then return nil end
    return { type = "text", text = c.text }
  elseif c.type == "thinking" then
    local b = { type = "thinking", thinking = c.thinking }
    if c.signature then b.signature = c.signature end
    return b
  elseif c.type == "image" then
    return { type = "image", data = c.data, mimeType = c.media_type }
  elseif c.type == "tool_call" then
    return { type = "toolCall", id = c.id, name = c.name, arguments = c.arguments or {} }
  elseif c.type == "tool_result" then
    return { type = "toolResult", toolCallId = c.id, content = c.content, isError = c.is_error }
  end
end

function M.request(model, ctx, opts)
  local messages = {}
  for _, m in ipairs(ctx.messages or {}) do
    local blocks = {}
    for _, c in ipairs(m.content or {}) do
      local b = block(c)
      if b then blocks[#blocks + 1] = b end
    end
    if #blocks > 0 then
      messages[#messages + 1] = { role = m.role, content = blocks }
    end
  end

  local body = {
    model = model.id,
    stream = true,
    messages = messages,
    maxTokens = math.min(opts.max_tokens or model.max_tokens, model.max_tokens),
  }
  if ctx.system then body.system = ctx.system end
  if ctx.tools and #ctx.tools > 0 then body.tools = ctx.tools end
  if opts.thinking and opts.thinking ~= "off" then body.thinking = opts.thinking end
  return body
end

local STOP = {
  toolUse = "tool_use", length = "length",
  stop = "end_turn", aborted = "aborted", error = "error",
}

function M.on_event(state, event)
  local ok, d = pcall(function() return axum.json.decode(event.data) end)
  if not ok or type(d) ~= "table" then return { scratch = state.scratch, usage = state.usage } end

  local deltas, usage = {}, state.usage
  local kind = d.type or event.name

  if kind == "text_delta" then
    deltas[#deltas + 1] = { kind = "text", text = d.delta or "" }
  elseif kind == "thinking_delta" then
    deltas[#deltas + 1] = { kind = "thinking", thinking = d.delta or "" }
  elseif kind == "thinking_end" then
    -- The signature arrives when the block closes, not with its text.
    if d.signature then
      deltas[#deltas + 1] = { kind = "signature", signature = d.signature }
    end
  elseif kind == "toolcall_start" then
    deltas[#deltas + 1] = { kind = "tool_call_start", id = d.id or "", name = d.toolName or "" }
  elseif kind == "toolcall_delta" then
    deltas[#deltas + 1] = { kind = "tool_call_args", arguments = d.delta or "" }
  elseif kind == "done" then
    local u = d.usage
    if u then
      usage = {
        input = u.input or 0, output = u.output or 0,
        cache_read = u.cacheRead or 0, cache_write = u.cacheWrite or 0,
      }
      deltas[#deltas + 1] = { kind = "usage", usage = usage }
    end
    deltas[#deltas + 1] = { kind = "stop", reason = STOP[d.stopReason] or "end_turn" }
  elseif kind == "error" then
    deltas[#deltas + 1] = { kind = "stop", reason = "error" }
  end

  return { scratch = state.scratch, usage = usage, deltas = deltas }
end

axum.api("pi-messages", M)
