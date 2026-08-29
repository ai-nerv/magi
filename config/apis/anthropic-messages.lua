-- Anthropic's Messages API.

local M = {}

-- Anthropic dates its protocol rather than numbering it.
local VERSION = "2023-06-01"

-- Reasoning budgets in tokens, for each level a caller can ask for. `off` is absent rather than
-- zero: the request omits thinking entirely, which is a different thing from asking for none.
local BUDGET = {
  minimal = 1024, low = 4096, medium = 16384, high = 32768, max = 63999,
}

function M.endpoint(base_url, _model)
  return base_url .. "/v1/messages"
end

function M.headers(key)
  local h = { ["anthropic-version"] = VERSION }
  -- Omitted rather than sent empty: an empty credential reads as a malformed request, and the
  -- transport already refuses to call when none was resolved.
  if key then h["x-api-key"] = key end
  return h
end

-- Blocks the API will accept back. A thinking block that lost its signature is dropped: the
-- API refuses one, so sending it buys a 400 instead of a turn.
local function block(c)
  if c.type == "text" then
    if c.text == "" then return nil end
    return { type = "text", text = c.text }
  elseif c.type == "thinking" then
    if not c.signature then return nil end
    return { type = "thinking", thinking = c.thinking, signature = c.signature }
  elseif c.type == "image" then
    return { type = "image",
             source = { type = "base64", media_type = c.media_type, data = c.data } }
  elseif c.type == "tool_call" then
    return { type = "tool_use", id = c.id, name = c.name, input = c.arguments }
  elseif c.type == "tool_result" then
    return { type = "tool_result", tool_use_id = c.id,
             content = c.content, is_error = c.is_error }
  end
end

-- Tool results are user-role blocks here rather than a role of their own, which is the largest
-- shape difference from the Completions dialect. Consecutive same-role messages are merged
-- because the API refuses two user turns in a row, and a result after a user message is that.
local function messages(list)
  local out = {}
  for _, m in ipairs(list or {}) do
    local role = (m.role == "assistant") and "assistant" or "user"
    local blocks = {}
    for _, c in ipairs(m.content or {}) do
      local b = block(c)
      if b then blocks[#blocks + 1] = b end
    end
    if #blocks > 0 then
      local last = out[#out]
      if last and last.role == role then
        for _, b in ipairs(blocks) do last.content[#last.content + 1] = b end
      else
        out[#out + 1] = { role = role, content = blocks }
      end
    end
  end
  return out
end

function M.request(model, ctx, opts)
  local body = {
    model = model.id,
    stream = true,
    max_tokens = math.min(opts.max_tokens or model.max_tokens, model.max_tokens),
    messages = messages(ctx.messages),
  }
  if ctx.system then body.system = ctx.system end

  -- Anthropic has no `response_format`. The idiom is a single tool the model is forced to call,
  -- whose input schema is the shape wanted -- so a schema request becomes exactly that, and the
  -- caller reads the answer out of the tool call rather than out of the text.
  if opts.schema then
    body.tools = {
      {
        name = opts.schema.name,
        description = "Answer by calling this with the requested value.",
        input_schema = opts.schema.schema,
      },
    }
    body.tool_choice = { type = "tool", name = opts.schema.name }
  elseif ctx.tools and #ctx.tools > 0 then
    local tools = {}
    for _, t in ipairs(ctx.tools) do
      tools[#tools + 1] = { name = t.name, description = t.description, input_schema = t.parameters }
    end
    body.tools = tools
  end

  -- A budget must leave room for a response: asking for the whole of max_tokens as reasoning
  -- yields a turn with nothing in it.
  local budget = opts.thinking and BUDGET[opts.thinking]
  if budget then
    local cap = math.max(model.max_tokens - 1024, 1024)
    body.thinking = { type = "enabled", budget_tokens = math.min(budget, cap) }
  end
  return body
end

local STOP = {
  tool_use = "tool_use", max_tokens = "length",
  end_turn = "end_turn", stop_sequence = "end_turn",
}

function M.on_event(state, event)
  local ok, d = pcall(function() return axum.json.decode(event.data) end)
  if not ok or type(d) ~= "table" then return { scratch = state.scratch, usage = state.usage } end

  local deltas = {}
  local usage = state.usage

  if event.name == "message_start" then
    local u = d.message and d.message.usage or {}
    usage = {
      input = u.input_tokens or 0,
      output = u.output_tokens or 0,
      cache_read = u.cache_read_input_tokens or 0,
      cache_write = u.cache_creation_input_tokens or 0,
    }
    deltas[#deltas + 1] = { kind = "usage", usage = usage }

  elseif event.name == "content_block_start" then
    local b = d.content_block or {}
    if b.type == "tool_use" then
      deltas[#deltas + 1] = { kind = "tool_call_start", id = b.id or "", name = b.name or "" }
    end

  elseif event.name == "content_block_delta" then
    local delta = d.delta or {}
    if delta.type == "text_delta" then
      deltas[#deltas + 1] = { kind = "text", text = delta.text or "" }
    elseif delta.type == "thinking_delta" then
      deltas[#deltas + 1] = { kind = "thinking", thinking = delta.thinking or "" }
    elseif delta.type == "signature_delta" then
      -- Arrives at the end of a thinking block and must be replayed verbatim for the model to
      -- accept its own reasoning back.
      deltas[#deltas + 1] = { kind = "signature", signature = delta.signature or "" }
    elseif delta.type == "input_json_delta" then
      deltas[#deltas + 1] = { kind = "tool_call_args", arguments = delta.partial_json or "" }
    end

  elseif event.name == "message_delta" then
    -- Output tokens are only final here, so the running total is replaced rather than added to:
    -- adding would double-count what message_start reported.
    if d.usage and d.usage.output_tokens then
      usage = {
        input = usage.input, cache_read = usage.cache_read,
        cache_write = usage.cache_write, output = d.usage.output_tokens,
      }
      deltas[#deltas + 1] = { kind = "usage", usage = usage }
    end
    local reason = d.delta and d.delta.stop_reason
    if reason then
      deltas[#deltas + 1] = { kind = "stop", reason = STOP[reason] or "end_turn" }
    end
  end

  return { scratch = state.scratch, usage = usage, deltas = deltas }
end

axum.api("anthropic-messages", M)
