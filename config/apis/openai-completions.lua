-- OpenAI's Chat Completions, and the twenty-odd vendors that copy it.
--
-- The protocol with the most identities behind it: most of the catalog speaks this, so most of
-- the catalog works the moment this file does. Where a vendor deviates it says so in its own
-- `compat` block rather than here — this describes the dialect, not the exceptions.

local M = {}

function M.endpoint(base_url, _model)
  return base_url .. "/chat/completions"
end

function M.headers(key)
  if not key then return {} end
  return { authorization = "Bearer " .. key }
end

-- One neutral message as one or more Completions messages.
--
-- The shape differences from Anthropic, all in one place: tool results are their own role and
-- carry the call id; tool calls hang off the assistant message rather than being content;
-- reasoning has no block of its own and rides in `reasoning_content`.
local function convert(m, out, compat)
  local text, tool_calls, results, reasoning = {}, {}, {}, nil

  for _, c in ipairs(m.content or {}) do
    if c.type == "text" then
      if c.text ~= "" then text[#text + 1] = c.text end
    elseif c.type == "thinking" then
      reasoning = (reasoning or "") .. (c.thinking or "")
    elseif c.type == "image" then
      text[#text + 1] = ""  -- images ride as parts below
    elseif c.type == "tool_call" then
      tool_calls[#tool_calls + 1] = {
        id = c.id, type = "function",
        ["function"] = { name = c.name, arguments = axum.json.encode(c.arguments or {}) },
      }
    elseif c.type == "tool_result" then
      local r = { role = "tool", tool_call_id = c.id, content = c.content }
      -- Some dialects reject a result that does not repeat the tool's name.
      if compat.requires_tool_result_name then r.name = c.name end
      results[#results + 1] = r
    end
  end

  for _, r in ipairs(results) do out[#out + 1] = r end
  if #results > 0 then return end

  local role = m.role
  if role == "assistant" then
    local msg = { role = "assistant", content = table.concat(text, "") }
    if #tool_calls > 0 then msg.tool_calls = tool_calls end
    -- A dialect that cannot carry reasoning gets it inline, delimited, rather than losing it.
    if reasoning then
      if compat.requires_thinking_as_text then
        msg.content = "<thinking>" .. reasoning .. "</thinking>" .. msg.content
      else
        msg.reasoning_content = reasoning
      end
    end
    out[#out + 1] = msg
  else
    out[#out + 1] = { role = "user", content = table.concat(text, "") }
  end
end

function M.request(model, ctx, opts)
  local compat = model.compat or {}
  local messages = {}

  if ctx.system then
    -- `developer` is an OpenAI extension most copies reject, so it is opted into.
    messages[#messages + 1] = {
      role = compat.supports_developer_role and "developer" or "system",
      content = ctx.system,
    }
  end
  for _, m in ipairs(ctx.messages or {}) do convert(m, messages, compat) end

  local body = { model = model.id, stream = true, messages = messages }

  -- Which field caps the response is the single most common way these dialects differ.
  local field = compat.max_tokens_field or "max_tokens"
  body[field] = math.min(opts.max_tokens or model.max_tokens, model.max_tokens)

  if compat.supports_store then body.store = false end
  -- Usage is not reported while streaming unless asked for, and some dialects cannot.
  if compat.supports_usage_in_streaming ~= false then
    body.stream_options = { include_usage = true }
  end

  if ctx.tools and #ctx.tools > 0 then
    local tools = {}
    for _, t in ipairs(ctx.tools) do
      tools[#tools + 1] = {
        type = "function",
        ["function"] = { name = t.name, description = t.description, parameters = t.parameters },
      }
    end
    body.tools = tools
  end

  -- Five vendors, five ways to ask for reasoning. The dialect is declared in the catalog.
  local level = opts.thinking
  if level and level ~= "off" then
    local format = compat.thinking_format or "openai"
    if format == "openrouter" then
      body.reasoning = { effort = level }
    elseif format == "deepseek" then
      body.thinking = { type = "enabled" }
      if compat.supports_reasoning_effort then body.reasoning_effort = level end
    elseif format == "zai" then
      body.thinking = { type = "enabled" }
    elseif format == "qwen" then
      body.enable_thinking = true
    elseif compat.supports_reasoning_effort then
      body.reasoning_effort = level
    end
  end
  return body
end

local FINISH = {
  tool_calls = "tool_use", length = "length",
  stop = "end_turn", content_filter = "end_turn",
}

function M.on_event(state, event)
  -- The stream ends with a literal sentinel rather than an event, which is this dialect's one
  -- genuine oddity: without it a clean end is indistinguishable from a dropped connection.
  if event.data == "[DONE]" then
    local scratch = state.scratch or {}
    if scratch.stopped then return { scratch = scratch, usage = state.usage } end
    return {
      scratch = scratch, usage = state.usage,
      deltas = { { kind = "stop", reason = scratch.saw_tool_call and "tool_use" or "end_turn" } },
    }
  end

  local ok, d = pcall(function() return axum.json.decode(event.data) end)
  if not ok or type(d) ~= "table" then return { scratch = state.scratch, usage = state.usage } end

  local scratch = state.scratch or {}
  local usage = state.usage
  local deltas = {}

  if d.usage then
    local u = d.usage
    local details = u.prompt_tokens_details or {}
    usage = {
      input = (u.prompt_tokens or 0) - (details.cached_tokens or 0),
      output = u.completion_tokens or 0,
      cache_read = details.cached_tokens or 0,
      cache_write = 0,
    }
    deltas[#deltas + 1] = { kind = "usage", usage = usage }
  end

  local choice = d.choices and d.choices[1]
  if choice then
    local delta = choice.delta or {}
    if delta.reasoning_content and delta.reasoning_content ~= "" then
      deltas[#deltas + 1] = { kind = "thinking", thinking = delta.reasoning_content }
    end
    if delta.content and delta.content ~= "" then
      deltas[#deltas + 1] = { kind = "text", text = delta.content }
    end
    for _, call in ipairs(delta.tool_calls or {}) do
      -- Calls arrive by index, and only the first chunk of each carries its id and name.
      local index = call.index or 0
      scratch.seen = scratch.seen or {}
      if not scratch.seen[tostring(index)] then
        scratch.seen[tostring(index)] = true
        scratch.saw_tool_call = true
        deltas[#deltas + 1] = {
          kind = "tool_call_start",
          id = call.id or "",
          name = call["function"] and call["function"].name or "",
        }
      end
      local args = call["function"] and call["function"].arguments
      if args and args ~= "" then
        deltas[#deltas + 1] = { kind = "tool_call_args", arguments = args }
      end
    end
    if choice.finish_reason then
      scratch.stopped = true
      deltas[#deltas + 1] = { kind = "stop", reason = FINISH[choice.finish_reason] or "end_turn" }
    end
  end

  return { scratch = scratch, usage = usage, deltas = deltas }
end

axum.api("openai-completions", M)
