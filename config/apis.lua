-- Every wire protocol magi speaks.
--
-- Each block is one dialect and stands alone: a `do ... end` so its locals stay its own,
-- ending in the `magi.api` calls that register it. A vendor deviating inside a dialect says
-- so in its `compat` block in `providers.lua`, not here.

do -- openai-completions
  local M = {}

  function M.endpoint(base_url, _model)
    return base_url .. "/chat/completions"
  end

  function M.headers(key)
    if not key then return {} end
    return { authorization = "Bearer " .. key }
  end

  -- One neutral message as one or more Completions messages.
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
          ["function"] = { name = c.name, arguments = magi.json.encode(c.arguments or {}) },
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
    -- Every field is answered before this file sees it: the host resolves the catalog's sparse
    -- `compat` against the conservative defaults. So `compat.x` is safe and `compat.x or <a
    -- default>` would be a second copy of a decision that already has a home.
    local compat = model.compat
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
    local field = compat.max_tokens_field
    body[field] = math.min(opts.max_tokens or model.max_tokens, model.max_tokens)

    if compat.supports_store then body.store = false end
    -- Usage is not reported while streaming unless asked for, and some dialects cannot.
    if compat.supports_usage_in_streaming then
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
    if opts.schema then
      body.response_format = {
        type = "json_schema",
        json_schema = { name = opts.schema.name, schema = opts.schema.schema, strict = true },
      }
    end

    local level = opts.thinking
    if level then
      local format = compat.thinking_format
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

    local ok, d = pcall(function() return magi.json.decode(event.data) end)
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
      -- Two spellings, because the dialects that added reasoning did not agree on one.
      -- DeepSeek and the copies of it say `reasoning_content`; OpenRouter says `reasoning`.
      -- Sending one vendor's request shape and reading only the other's reply shape is how
      local reasoned = delta.reasoning_content
      if type(reasoned) ~= "string" or reasoned == "" then
        reasoned = type(delta.reasoning) == "string" and delta.reasoning or nil
      end
      if reasoned and reasoned ~= "" then
        deltas[#deltas + 1] = { kind = "thinking", thinking = reasoned }
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

  magi.api("openai-completions", M)
end

do -- openai-responses
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
            arguments = magi.json.encode(c.arguments or {}),
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
    local ok, d = pcall(function() return magi.json.decode(event.data) end)
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
  magi.api("openai-responses", openai)

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
  magi.api("azure-openai-responses", azure)

  local codex = {}
  for k, v in pairs(M) do codex[k] = v end
  function codex.endpoint(base_url, _model) return base_url .. "/codex/responses" end
  function codex.headers(key)
    if not key then return {} end
    -- A subscription token, not an API key: the account is the credential, which is why this
    -- provider declares `oauth` rather than naming a variable to export.
    return { authorization = "Bearer " .. key, ["openai-beta"] = "responses=experimental" }
  end
  magi.api("openai-codex-responses", codex)
end

do -- anthropic-messages
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
    local ok, d = pcall(function() return magi.json.decode(event.data) end)
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

  magi.api("anthropic-messages", M)
end

do -- google
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
    -- Google puts it on the generation config, and wants the mime type set with it.
    if opts.schema then
      body.generationConfig = body.generationConfig or {}
      body.generationConfig.responseMimeType = "application/json"
      body.generationConfig.responseSchema = opts.schema.schema
    end

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
    local ok, d = pcall(function() return magi.json.decode(event.data) end)
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
            arguments = magi.json.encode(p.functionCall.args or {}),
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
  magi.api("google-generative-ai", gemini)

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
  magi.api("google-vertex", vertex)
end

do -- pi-messages
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
    local ok, d = pcall(function() return magi.json.decode(event.data) end)
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

  magi.api("pi-messages", M)
end
