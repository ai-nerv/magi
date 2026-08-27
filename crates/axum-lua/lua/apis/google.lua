-- Google's Generative Language API, and Vertex, which is the same protocol behind a different
-- door: a project-scoped endpoint and application default credentials instead of a key.

local M = {}

-- Google names the model's turn "model" rather than "assistant", and carries tool calls and
-- results as parts inside a turn rather than as messages of their own.
local function part(c)
  if c.type == "text" then
    if c.text == "" then return nil end
    return { text = c.text }
  elseif c.type == "thinking" then
    if c.thinking == "" then return nil end
    -- Reasoning is a text part flagged as thought; the signature rides beside it and must be
    -- replayed for the model to accept its own reasoning back.
    local p = { text = c.thinking, thought = true }
    if c.signature then p.thoughtSignature = c.signature end
    return p
  elseif c.type == "image" then
    return { inlineData = { mimeType = c.media_type, data = c.data } }
  elseif c.type == "tool_call" then
    local p = { functionCall = { name = c.name, args = c.arguments or {} } }
    -- Google issues a signature per call rather than per message.
    if c.thought_signature then p.thoughtSignature = c.thought_signature end
    return p
  elseif c.type == "tool_result" then
    return { functionResponse = { name = c.name, response = { output = c.content } } }
  end
end

local function contents(messages)
  local out = {}
  for _, m in ipairs(messages or {}) do
    local role = (m.role == "assistant") and "model" or "user"
    local parts = {}
    for _, c in ipairs(m.content or {}) do
      local p = part(c)
      if p then parts[#parts + 1] = p end
    end
    if #parts > 0 then
      local last = out[#out]
      -- Turns must alternate, so consecutive same-role content is merged rather than sent as
      -- two turns the API will refuse.
      if last and last.role == role then
        for _, p in ipairs(parts) do last.parts[#last.parts + 1] = p end
      else
        out[#out + 1] = { role = role, parts = parts }
      end
    end
  end
  return out
end

-- Thinking budgets in tokens. `-1` is Google's "decide for yourself", which is what a caller
-- asking for `max` actually wants: the model knows its own ceiling better than we do.
local BUDGET = { minimal = 512, low = 2048, medium = 8192, high = 24576, max = -1 }

function M.request(model, ctx, opts)
  local body = {
    contents = contents(ctx.messages),
    generationConfig = {
      maxOutputTokens = math.min(opts.max_tokens or model.max_tokens, model.max_tokens),
    },
  }
  if ctx.system then
    body.systemInstruction = { parts = { { text = ctx.system } } }
  end

  if ctx.tools and #ctx.tools > 0 then
    local declarations = {}
    for _, t in ipairs(ctx.tools) do
      declarations[#declarations + 1] = {
        name = t.name, description = t.description, parameters = t.parameters,
      }
    end
    body.tools = { { functionDeclarations = declarations } }
  end

  local level = opts.thinking
  if level and level ~= "off" then
    body.generationConfig.thinkingConfig = {
      thinkingBudget = BUDGET[level] or -1,
      includeThoughts = true,
    }
  end
  return body
end

local FINISH = {
  STOP = "end_turn", MAX_TOKENS = "length",
  SAFETY = "end_turn", RECITATION = "end_turn",
}

function M.on_event(state, event)
  local ok, d = pcall(function() return axum.json.decode(event.data) end)
  if not ok or type(d) ~= "table" then return { scratch = state.scratch, usage = state.usage } end

  local deltas, usage = {}, state.usage
  local scratch = state.scratch or {}

  local u = d.usageMetadata
  if u then
    usage = {
      input = (u.promptTokenCount or 0) - (u.cachedContentTokenCount or 0),
      output = u.candidatesTokenCount or 0,
      cache_read = u.cachedContentTokenCount or 0,
      cache_write = 0,
    }
    deltas[#deltas + 1] = { kind = "usage", usage = usage }
  end

  local candidate = d.candidates and d.candidates[1]
  if candidate then
    for _, p in ipairs(candidate.content and candidate.content.parts or {}) do
      if p.functionCall then
        scratch.saw_tool_call = true
        deltas[#deltas + 1] = {
          kind = "tool_call_start",
          -- Google does not issue call ids, so the name stands in: a turn with two calls to
          -- the same tool is the case this cannot tell apart, and the API does not either.
          id = p.functionCall.name or "",
          name = p.functionCall.name or "",
        }
        deltas[#deltas + 1] = {
          kind = "tool_call_args",
          arguments = axum.json.encode(p.functionCall.args or {}),
        }
      elseif p.thought then
        deltas[#deltas + 1] = { kind = "thinking", thinking = p.text or "" }
        if p.thoughtSignature then
          deltas[#deltas + 1] = { kind = "signature", signature = p.thoughtSignature }
        end
      elseif p.text then
        deltas[#deltas + 1] = { kind = "text", text = p.text }
      end
    end

    if candidate.finishReason then
      local reason = FINISH[candidate.finishReason] or "end_turn"
      -- A turn that produced a call has not ended, whatever the finish reason says.
      if scratch.saw_tool_call and reason == "end_turn" then reason = "tool_use" end
      deltas[#deltas + 1] = { kind = "stop", reason = reason }
    end
  end

  return { scratch = scratch, usage = usage, deltas = deltas }
end

-- ---------------------------------------------------------------- the two hostings

local gemini = {}
for k, v in pairs(M) do gemini[k] = v end
-- `alt=sse` is what turns this into a stream; without it the endpoint answers one JSON array
-- at the end, which looks like a very slow model.
function gemini.endpoint(base_url, model)
  return base_url .. "/models/" .. model.id .. ":streamGenerateContent?alt=sse"
end
function gemini.headers(key)
  if not key then return {} end
  return { ["x-goog-api-key"] = key }
end
axum.api("google-generative-ai", gemini)

local vertex = {}
for k, v in pairs(M) do vertex[k] = v end
-- Vertex scopes by project and region, which the base URL carries; the model is a publisher
-- path rather than a bare id.
function vertex.endpoint(base_url, model)
  return base_url .. "/publishers/google/models/" .. model.id .. ":streamGenerateContent?alt=sse"
end
function vertex.headers(key)
  if not key then return {} end
  -- A short-lived access token from the credential chain, not an API key.
  return { authorization = "Bearer " .. key }
end
axum.api("google-vertex", vertex)
