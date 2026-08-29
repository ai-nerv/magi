-- OpenAI's Responses API, and the two hostings that differ only in where and how you knock.

local M = {}

-- Input items, which is Responses' name for the conversation. Tool calls and their results are
-- items of their own rather than blocks inside a message, which is the shape difference from
-- Completions that catches people out.
local function inputs(messages)
  local out = {}
  for _, m in ipairs(messages or {}) do
    local text, calls, results = {}, {}, {}
    for _, c in ipairs(m.content or {}) do
      if c.type == "text" then
        if c.text ~= "" then text[#text + 1] = c.text end
      elseif c.type == "tool_call" then
        calls[#calls + 1] = {
          type = "function_call", call_id = c.id, name = c.name,
          arguments = axum.json.encode(c.arguments or {}),
        }
      elseif c.type == "tool_result" then
        results[#results + 1] = {
          type = "function_call_output", call_id = c.id, output = c.content,
        }
      end
    end

    for _, r in ipairs(results) do out[#out + 1] = r end
    if #results == 0 and #text > 0 then
      local role = (m.role == "assistant") and "assistant" or "user"
      -- The content part is named for its direction: what the model produced is `output_text`
      -- and what it was given is `input_text`, and swapping them is rejected.
      local part = (role == "assistant") and "output_text" or "input_text"
      out[#out + 1] = {
        type = "message", role = role,
        content = { { type = part, text = table.concat(text, "") } },
      }
    end
    for _, c in ipairs(calls) do out[#out + 1] = c end
  end
  return out
end

function M.request(model, ctx, opts)
  local body = {
    model = model.id,
    stream = true,
    input = inputs(ctx.messages),
    max_output_tokens = math.min(opts.max_tokens or model.max_tokens, model.max_tokens),
  }
  if ctx.system then body.instructions = ctx.system end

  -- Responses moved it under `text.format`, and flattened the schema up a level.
  if opts.schema then
    body.text = {
      format = {
        type = "json_schema",
        name = opts.schema.name,
        schema = opts.schema.schema,
        strict = true,
      },
    }
  end

  if ctx.tools and #ctx.tools > 0 then
    local tools = {}
    for _, t in ipairs(ctx.tools) do
      tools[#tools + 1] = {
        type = "function", name = t.name,
        description = t.description, parameters = t.parameters,
      }
    end
    body.tools = tools
  end

  local level = opts.thinking
  if level and level ~= "off" then
    body.reasoning = { effort = level, summary = "auto" }
  end
  return body
end

local INCOMPLETE = { max_output_tokens = "length" }

function M.on_event(state, event)
  local ok, d = pcall(function() return axum.json.decode(event.data) end)
  if not ok or type(d) ~= "table" then return { scratch = state.scratch, usage = state.usage } end

  local deltas, usage = {}, state.usage
  local kind = d.type or event.name

  if kind == "response.output_text.delta" then
    deltas[#deltas + 1] = { kind = "text", text = d.delta or "" }

  elseif kind == "response.reasoning_text.delta"
      or kind == "response.reasoning_summary_text.delta" then
    deltas[#deltas + 1] = { kind = "thinking", thinking = d.delta or "" }

  elseif kind == "response.output_item.added" then
    local item = d.item or {}
    if item.type == "function_call" then
      deltas[#deltas + 1] = {
        kind = "tool_call_start", id = item.call_id or item.id or "", name = item.name or "",
      }
    end

  elseif kind == "response.function_call_arguments.delta" then
    deltas[#deltas + 1] = { kind = "tool_call_args", arguments = d.delta or "" }

  elseif kind == "response.completed" or kind == "response.incomplete"
      or kind == "response.failed" then
    local r = d.response or {}
    local u = r.usage
    if u then
      local details = u.input_tokens_details or {}
      usage = {
        input = (u.input_tokens or 0) - (details.cached_tokens or 0),
        output = u.output_tokens or 0,
        cache_read = details.cached_tokens or 0,
        cache_write = 0,
      }
      deltas[#deltas + 1] = { kind = "usage", usage = usage }
    end

    local reason = "end_turn"
    if kind == "response.incomplete" then
      -- An incomplete response says why, and running out of room is the case that must poison
      -- the turn's tool calls rather than look like a clean finish.
      local why = r.incomplete_details and r.incomplete_details.reason
      reason = INCOMPLETE[why] or "end_turn"
    elseif kind == "response.failed" then
      reason = "error"
    else
      -- A completed response that produced a call is waiting on it.
      for _, item in ipairs(r.output or {}) do
        if item.type == "function_call" then reason = "tool_use" end
      end
    end
    deltas[#deltas + 1] = { kind = "stop", reason = reason }
  end

  return { scratch = state.scratch, usage = usage, deltas = deltas }
end

-- ---------------------------------------------------------------- the three hostings

local openai = {}
for k, v in pairs(M) do openai[k] = v end
function openai.endpoint(base_url, _model) return base_url .. "/responses" end
function openai.headers(key)
  if not key then return {} end
  return { authorization = "Bearer " .. key }
end
axum.api("openai-responses", openai)

local azure = {}
for k, v in pairs(M) do azure[k] = v end
-- Azure routes by deployment and dates its API in the query string; the base URL carries the
-- resource, so the model id is the deployment name.
function azure.endpoint(base_url, model)
  return base_url .. "/openai/deployments/" .. model.id .. "/responses?api-version=2025-04-01-preview"
end
function azure.headers(key)
  if not key then return {} end
  return { ["api-key"] = key }
end
axum.api("azure-openai-responses", azure)

local codex = {}
for k, v in pairs(M) do codex[k] = v end
function codex.endpoint(base_url, _model) return base_url .. "/codex/responses" end
function codex.headers(key)
  if not key then return {} end
  -- A subscription token, not an API key: the account is the credential, which is why this
  -- provider declares `oauth` rather than naming a variable to export.
  return { authorization = "Bearer " .. key, ["openai-beta"] = "responses=experimental" }
end
axum.api("openai-codex-responses", codex)
