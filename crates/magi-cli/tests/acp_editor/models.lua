return function(ctx)
  local options, completed = ctx.session.config_options, ctx.completed
  local function pick(value, artifact)
    local current, ordered = options.model.currentValue, {}
    for _, option in ipairs(options.model.options) do
      if option.value == current then ordered[#ordered + 1] = option end
    end
    for _, option in ipairs(options.model.options) do
      if option.value ~= current then ordered[#ordered + 1] = option end
    end
    local index
    for number, option in ipairs(ordered) do if option.value == value then index = number end end
    assert(index, "offered model " .. value)
    vim.fn.writefile({ tostring(index) }, "../" .. artifact .. ".ready")
    ctx.wait_for("external picker driver", function() return vim.fn.filereadable("../" .. artifact .. ".armed") == 1 end)
    assert(options:_show_model_selector(), "actual model picker displayed and selected")
  end
  local function rejected(value)
    ctx.wait_for("rejected model " .. value, function()
      return vim.api.nvim_exec2("messages", { output = true }).output:find("Failed to change Model to '" .. value .. "'", 1, true) ~= nil
    end)
  end
  ctx.wait_for("initial model", function() return options:get_model_id() == "fake/one" end)
  ctx.submit("editor-model-busy")
  ctx.wait_for("held initial model prompt", function() return ctx.chat():find("part-0", 1, true) ~= nil end)
  assert(ctx.session.is_generating and #completed == 0)
  pick("fake/two", "model-busy-picker")
  rejected("fake/two")
  assert(options:get_model_id() == "fake/one", "busy refusal preserves model")
  ctx.agentic.stop_generation()
  ctx.wait_for("busy prompt cancelled", function() return #completed == 1 end)
  assert(completed[1].success)
  assert(ctx.chat():find("Generation stopped by the user request", 1, true))
  pick("fake/two", "model-select-picker")
  ctx.wait_for("idle selection acknowledged", function() return options:get_model_id() == "fake/two" end)
  ctx.wait_for("idle selection response", function()
    return vim.api.nvim_exec2("messages", { output = true }).output:find("Model changed to: fake/two", 1, true) ~= nil
  end)
  vim.fn.writefile({ "selected-without-model-call" }, "../model-selected-ready")
  ctx.wait_for("native selection check", function() return vim.fn.filereadable("../model-selected-checked") == 1 end)
  ctx.submit("editor-selected-model")
  ctx.wait_for("selected model prefix", function() return ctx.chat():find("part-1", 1, true) ~= nil end)
  ctx.wait_for("selected model header", function()
    return vim.wo[ctx.session.widget.win_nrs.chat].winbar:find("fake/two", 1, true) ~= nil
  end)
  ctx.screen("model-selected-screen.txt", "fake/two")
  vim.fn.writefile({ "selected-model-prefix" }, "../switched-prompt-ready")
  ctx.wait_for("selected model completion", function() return #completed == 2 end)
  assert(completed[2].success)
  pick("fake/unavailable", "model-refuse-picker")
  rejected("fake/unavailable")
  assert(options:get_model_id() == "fake/two", "unavailable refusal preserves model")
  ctx.submit("editor-model-after-refusal")
  ctx.wait_for("model after refusal prefix", function() return ctx.chat():find("part-2", 1, true) ~= nil end)
  vim.fn.writefile({ "refusal-model-prefix" }, "../refusal-prompt-ready")
  ctx.wait_for("model after refusal completion", function() return #completed == 3 end)
  assert(completed[3].success)
  ctx.screen("model-recovered-screen.txt", "part-2-done")
  return { selected_model = options:get_model_id(), busy_change_refused = true, unavailable_change_refused = true }
end
